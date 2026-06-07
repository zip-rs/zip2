//! Types that specify what is contained in a ZIP.

use crate::CompressionMethod;
use crate::cp437::FromCp437;
use crate::datetime::DateTime;
use crate::extra_fields::ExtraFields;
use crate::format::flags::ZipFlags;
use crate::path::{enclosed_name, file_name_sanitized};
use crate::read::readers::SeekableTake;
use crate::result::{ZipError, ZipResult, invalid};
use crate::spec::is_dir;
use crate::spec::{
    self, FixedSizeBlock, Magic, Zip64DataDescriptorBlock, ZipDataDescriptorBlock,
    ZipLocalEntryBlock,
};
use crate::write::FileOptionExtension;
use crate::zipcrypto::ZipCryptoKeys;
use core::marker::PhantomData;
use std::borrow::Cow;
use std::ffi::OsStr;
use std::io::{Read, Seek, SeekFrom, Take};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

pub(crate) use crate::format::aes::{AesMode, AesVendorVersion};
pub(crate) use crate::format::flags::System;

pub(crate) mod ffi {
    pub const S_IFDIR: u32 = 0o0_040_000;
    pub const S_IFREG: u32 = 0o0_100_000;
    pub const S_IFLNK: u32 = 0o0_120_000;
}

pub(crate) struct ZipRawValues {
    pub(crate) crc32: u32,
    pub(crate) compressed_size: u64,
    pub(crate) uncompressed_size: u64,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum EncryptWith<'k> {
    #[cfg(feature = "aes-crypto")]
    Aes {
        mode: crate::AesMode,
        vendor_version: AesVendorVersion,
        // When the password is None, it means that we are reusing the previous encryption
        password: Option<&'k [u8]>,
        salt: Option<crate::aes::AesSalt>,
    },
    ZipCrypto(ZipCryptoKeys, PhantomData<&'k ()>),
}

#[cfg(feature = "_arbitrary")]
impl<'a> arbitrary::Arbitrary<'a> for EncryptWith<'a> {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        #[cfg(feature = "aes-crypto")]
        if bool::arbitrary(u)? {
            return Ok(EncryptWith::Aes {
                mode: crate::AesMode::arbitrary(u)?,
                password: Some(u.arbitrary::<&[u8]>()?),
                vendor_version: AesVendorVersion::Ae2,
                salt: None, // We don't need to test with random salt. It's only for testing or reproducible zips
            });
        }

        Ok(EncryptWith::ZipCrypto(
            ZipCryptoKeys::arbitrary(u)?,
            PhantomData,
        ))
    }
}

/// Metadata for a file to be written
#[non_exhaustive]
#[derive(Clone, Debug, Copy, Eq, PartialEq)]
pub struct FileOptions<'k, 'n, T: FileOptionExtension> {
    pub(crate) compression_method: CompressionMethod,
    pub(crate) compression_level: Option<i64>,
    pub(crate) last_modified_time: DateTime,
    pub(crate) permissions: Option<u32>,
    pub(crate) large_file: bool,
    pub(crate) encrypt_with: Option<EncryptWith<'k>>,
    pub(crate) extended_options: T,
    pub(crate) alignment: u16,
    #[cfg(feature = "deflate-zopfli")]
    pub(super) zopfli_buffer_size: Option<usize>,
    pub(crate) system: Option<System>,
    pub(crate) name: Option<&'n [u8]>,
}
/// Simple File Options. Can be copied and good for simple writing zip files
pub type SimpleFileOptions = FileOptions<'static, 'static, ()>;

impl FileOptions<'static, 'static, ()> {
    const DEFAULT_FILE_PERMISSION: u32 = 0o100_644;
}

pub const MIN_VERSION: u8 = 10;
pub const DEFAULT_VERSION: u8 = 45;

/// Structure representing a ZIP file.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ZipFileData {
    /// Compatibility of the file attribute information
    pub system: System,
    /// Specification version
    pub version_made_by: u8,
    /// ZIP flags
    pub flags: u16,
    /// Compression method used to store the file (get the inner compression method if encryption is used)
    pub compression_method: CompressionMethod,
    /// Last modified time. This will only have a 2 second precision.
    pub last_modified_time: Option<DateTime>,
    /// CRC32 checksum
    pub crc32: u32,
    /// Size of the file in the ZIP
    pub compressed_size: u64,
    /// Size of the file when extracted
    pub uncompressed_size: u64,
    /// File comment
    pub file_comment: Box<str>,
    /// Specifies where the local header of the file starts
    pub header_start: u64,
    /// Specifies where the extra data of the file starts
    pub extra_data_start: Option<u64>,
    /// Specifies where the central header of the file starts
    ///
    /// Note that when this is not known, it is set to 0
    pub central_header_start: u64,
    /// Specifies where the compressed data of the file starts
    pub data_start: OnceLock<u64>,
    /// External file attributes
    pub external_attributes: u32,
    /// Reserve local ZIP64 extra field
    pub large_file: bool,
    /// AES settings if applicable
    pub aes_mode: Option<(AesMode, AesVendorVersion)>,
    /// extra fields, see <https://libzip.org/specifications/extrafld.txt>
    pub extra_fields: ExtraFields,
}

impl ZipFileData {
    pub(crate) fn name<'a>(&self, file_name_raw: &'a [u8]) -> ZipResult<Cow<'a, str>> {
        Ok(
            if let Ok(file_name_utf8) = std::str::from_utf8(file_name_raw) {
                file_name_utf8.into()
            } else {
                file_name_raw.from_cp437().map_err(std::io::Error::other)?
            },
        )
    }

    /// Check if the encrypted flag is set
    pub fn is_encrypted(&self) -> bool {
        ZipFlags::matching(self.flags, ZipFlags::Encrypted)
    }

    /// Check if the data descriptor flag is set
    pub fn is_using_data_descriptor(&self) -> bool {
        ZipFlags::matching(self.flags, ZipFlags::UsingDataDescriptor)
    }

    /// Get the starting offset of the data of the compressed file
    pub fn data_start(&self, reader: &mut (impl Read + Seek + ?Sized)) -> ZipResult<u64> {
        match self.data_start.get() {
            Some(data_start) => Ok(*data_start),
            None => Ok(self.find_data_start(reader)?),
        }
    }

    pub(crate) fn find_data_start(
        &self,
        reader: &mut (impl Read + Seek + ?Sized),
    ) -> Result<u64, ZipError> {
        // Go to start of data.
        reader.seek(SeekFrom::Start(self.header_start))?;

        // Parse static-sized fields and check the magic value.
        let block = ZipLocalEntryBlock::parse(reader)?;

        // Calculate the end of the local header from the fields we just parsed.
        let variable_fields_len =
        // Each of these fields must be converted to u64 before adding, as the result may
        // easily overflow a u16.
        u64::from(block.file_name_length) + u64::from(block.extra_field_length);
        let data_start = self.header_start
            + (size_of::<Magic>() + size_of::<ZipLocalEntryBlock>()) as u64
            + variable_fields_len;

        // Set the value so we don't have to read it again.
        match self.data_start.set(data_start) {
            Ok(()) => (),
            // If the value was already set in the meantime, ensure it matches (this is probably
            // unnecessary).
            Err(existing_value) => {
                debug_assert_eq!(existing_value, data_start);
            }
        }

        Ok(data_start)
    }

    pub(crate) fn find_content<'a, R: Read + Seek + ?Sized>(
        &self,
        reader: &'a mut R,
    ) -> ZipResult<Take<&'a mut R>> {
        // TODO: use .get_or_try_init() once stabilized to provide a closure returning a Result!
        let data_start = self.data_start(reader)?;
        reader.seek(SeekFrom::Start(data_start))?;

        Ok(reader.take(self.compressed_size))
    }

    pub(crate) fn find_content_seek<'a, R: Read + Seek + ?Sized>(
        &self,
        reader: &'a mut R,
    ) -> ZipResult<SeekableTake<'a, R>> {
        // Parse local header
        let data_start = self.data_start(reader)?;
        reader.seek(SeekFrom::Start(data_start))?;

        // Explicit Ok and ? are needed to convert io::Error to ZipError
        Ok(SeekableTake::new(reader, self.compressed_size)?)
    }

    /// Check if the file is a directory based on the file name.
    pub fn is_dir(&self, file_name: &[u8]) -> bool {
        is_dir(file_name)
    }

    pub(crate) fn file_name_sanitized(&self, file_name: &str) -> PathBuf {
        let no_null_filename = match file_name.find('\0') {
            Some(index) => &file_name[0..index],
            None => file_name,
        };

        file_name_sanitized(no_null_filename)
    }

    /// Simplify the file name by removing the prefix and parent directories and only return normal components
    pub(crate) fn simplified_components<'a>(&self, file_name: &'a str) -> Option<Vec<&'a OsStr>> {
        if file_name.contains('\0') {
            return None;
        }
        let input: &'a Path = Path::new(file_name);
        crate::path::simplified_components(input)
    }

    pub(crate) fn enclosed_name(&self, file_name: &str) -> Option<PathBuf> {
        if file_name.contains('\0') {
            return None;
        }
        let enclosed = enclosed_name(file_name)?;
        Some(enclosed)
    }

    /// Get unix mode for the file
    pub(crate) const fn unix_mode(&self) -> Option<u32> {
        if self.external_attributes == 0 {
            return None;
        }
        let unix_mode = self.external_attributes >> 16;
        if unix_mode != 0 {
            // If the high 16 bits are non-zero, they probably contain Unix permissions.
            // This happens for archives created on Windows by this crate or other tools,
            // and is the only way to identify symlinks in such archives.
            return Some(unix_mode);
        }
        match self.system {
            System::Unix => Some(unix_mode),
            System::Dos => {
                // Interpret MS-DOS directory bit
                let mut mode = if 0x10 == (self.external_attributes & 0x10) {
                    ffi::S_IFDIR | 0o0775
                } else {
                    ffi::S_IFREG | 0o0664
                };
                if 0x01 == (self.external_attributes & 0x01) {
                    // Read-only bit; strip write permissions
                    mode &= !0o222;
                }
                Some(mode)
            }
            _ => None,
        }
    }

    /// PKZIP version needed to open this file (from APPNOTE 4.4.3.2).
    pub fn version_needed(&self) -> u16 {
        let compression_version: u16 = match self.compression_method {
            CompressionMethod::Stored => MIN_VERSION.into(),
            #[cfg(feature = "_deflate-any")]
            CompressionMethod::Deflated => 20,
            #[cfg(feature = "_bzip2_any")]
            CompressionMethod::Bzip2 => 46,
            #[cfg(feature = "deflate64")]
            CompressionMethod::Deflate64 => 21,
            #[cfg(feature = "lzma")]
            CompressionMethod::Lzma => 63,
            #[cfg(feature = "xz")]
            CompressionMethod::Xz => 63,
            // APPNOTE doesn't specify a version for Zstandard
            _ => u16::from(DEFAULT_VERSION),
        };
        let crypto_version: u16 = if self.aes_mode.is_some() {
            51
        } else if self.is_encrypted() {
            20
        } else {
            10
        };
        let misc_feature_version: u16 = if self.large_file {
            45
        } else if self
            .unix_mode()
            .is_some_and(|mode| mode & ffi::S_IFDIR == ffi::S_IFDIR)
        {
            // file is directory
            20
        } else {
            10
        };
        compression_version
            .max(crypto_version)
            .max(misc_feature_version)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn initialize_local_block<T: FileOptionExtension>(
        file_name_raw: &[u8],
        options: &FileOptions<'_, '_, T>,
        raw_values: &ZipRawValues,
        header_start: u64,
        extra_data_start: Option<u64>,
        compression_method: CompressionMethod,
        aes_settings: Option<(AesMode, AesVendorVersion)>,
        extra_fields: ExtraFields,
    ) -> Self {
        let permissions = options
            .permissions
            .unwrap_or(FileOptions::DEFAULT_FILE_PERMISSION);
        let mut external_attributes = permissions << 16;
        let system = if (permissions & ffi::S_IFLNK) == ffi::S_IFLNK {
            System::Unix
        } else if let Some(system_option) = options.system {
            // user provided
            system_option
        } else if cfg!(windows) {
            System::Dos
        } else {
            System::Unix
        };
        if system == System::Dos {
            if is_dir(file_name_raw) {
                // DOS directory bit
                external_attributes |= 0x10;
            }
            if options
                .permissions
                .is_some_and(|permissions| permissions & 0o444 == 0)
            {
                // DOS read-only bit
                external_attributes |= 0x01;
            }
        }
        let mut flags = 0;
        if options.has_encryption() {
            // encrypt_with is AES or ZipCrypto
            flags |= ZipFlags::Encrypted.as_u16();
        }
        if std::str::from_utf8(file_name_raw).is_ok() && !file_name_raw.is_ascii() {
            flags |= ZipFlags::LanguageEncoding.as_u16();
        }
        let mut local_block = ZipFileData {
            system,
            version_made_by: DEFAULT_VERSION,
            flags,
            compression_method,
            last_modified_time: Some(options.last_modified_time),
            crc32: raw_values.crc32,
            compressed_size: raw_values.compressed_size,
            uncompressed_size: raw_values.uncompressed_size,
            file_comment: String::with_capacity(0).into_boxed_str(),
            header_start,
            data_start: OnceLock::new(),
            central_header_start: 0,
            external_attributes,
            large_file: options.large_file,
            aes_mode: aes_settings,
            extra_fields,
            extra_data_start,
        };
        local_block.version_made_by = local_block.version_needed() as u8;
        local_block
    }

    pub(crate) fn from_local_block(
        block: ZipLocalEntryBlock,
        extra_fields: ExtraFields,
    ) -> ZipResult<Self> {
        let ZipLocalEntryBlock {
            version_made_by,
            flags,
            compression_method,
            last_mod_time,
            last_mod_date,
            crc32,
            compressed_size,
            uncompressed_size,
            ..
        } = block;

        let compression_method = CompressionMethod::parse_from_u16(compression_method);
        let (version_made_by, system) = System::extract_bytes(version_made_by);
        let data = ZipFileData {
            system,
            version_made_by,
            flags,
            compression_method,
            last_modified_time: DateTime::try_from_msdos(last_mod_date, last_mod_time).ok(),
            crc32,
            compressed_size: compressed_size.into(),
            uncompressed_size: uncompressed_size.into(),
            file_comment: String::with_capacity(0).into_boxed_str(), // file comment is only available in the central directory
            // header_start and data start are not available, but also don't matter, since seeking is
            // not available.
            header_start: 0,
            data_start: OnceLock::new(),
            central_header_start: 0,
            // The external_attributes field is only available in the central directory.
            // We set this to zero, which should be valid as the docs state 'If input came
            // from standard input, this field is set to zero.'
            external_attributes: 0,
            large_file: false,
            aes_mode: None,
            extra_fields,
            extra_data_start: None,
        };
        Ok(data)
    }

    pub(crate) fn flags(&self, file_name_raw: &[u8]) -> u16 {
        let is_utf8 = std::str::from_utf8(file_name_raw).is_ok();
        let is_ascii = file_name_raw.is_ascii() && self.file_comment.is_ascii();
        let utf8_bit: u16 = if is_utf8 && !is_ascii {
            ZipFlags::LanguageEncoding.as_u16()
        } else {
            0
        };

        let using_data_descriptor_bit = if self.is_using_data_descriptor() {
            ZipFlags::UsingDataDescriptor.as_u16()
        } else {
            0
        };

        let encrypted_bit: u16 = if self.is_encrypted() { 1u16 << 0 } else { 0 };

        utf8_bit | using_data_descriptor_bit | encrypted_bit
    }

    pub(crate) fn clamp_size_field(&self, field: u64) -> Result<u32, std::io::Error> {
        if self.large_file {
            Ok(spec::ZIP64_BYTES_THR as u32)
        } else {
            field.min(spec::ZIP64_BYTES_THR).try_into().map_err(|_| {
                std::io::Error::other(format!(
                    "File size {field} exceeds maximum size for non-ZIP64 files"
                ))
            })
        }
    }

    pub(crate) fn write_data_descriptor<W: std::io::Write>(
        &self,
        writer: &mut W,
        auto_large_file: bool,
    ) -> Result<(), ZipError> {
        if self.large_file {
            return self.zip64_data_descriptor_block().write(writer);
        }
        if self.compressed_size > spec::ZIP64_BYTES_THR
            || self.uncompressed_size > spec::ZIP64_BYTES_THR
        {
            if auto_large_file {
                return self.zip64_data_descriptor_block().write(writer);
            }
            return Err(ZipError::Io(std::io::Error::other(
                "Large file option has not been set - use .large_file(true) in options",
            )));
        }
        self.data_descriptor_block().write(writer)
    }

    pub(crate) fn data_descriptor_block(&self) -> ZipDataDescriptorBlock {
        ZipDataDescriptorBlock {
            crc32: self.crc32,
            compressed_size: self.compressed_size as u32,
            uncompressed_size: self.uncompressed_size as u32,
        }
    }

    pub(crate) fn zip64_data_descriptor_block(&self) -> Zip64DataDescriptorBlock {
        Zip64DataDescriptorBlock {
            crc32: self.crc32,
            compressed_size: self.compressed_size,
            uncompressed_size: self.uncompressed_size,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn system() {
        use super::System;
        assert_eq!(u8::from(System::Dos), 0u8);
        assert_eq!(System::Dos as u8, 0u8);
        assert_eq!(System::Unix as u8, 3u8);
        assert_eq!(u8::from(System::Unix), 3u8);
        assert_eq!(System::from(0), System::Dos);
        assert_eq!(System::from(3), System::Unix);
        assert_eq!(u8::from(System::Unknown), 255u8);
        assert_eq!(System::Unknown as u8, 255u8);
    }

    #[test]
    fn unix_mode_robustness() {
        use super::{System, ZipFileData};
        use crate::types::ffi;
        let mut data = ZipFileData {
            system: System::Dos,
            external_attributes: (ffi::S_IFLNK | 0o777) << 16,
            ..ZipFileData::default()
        };
        assert_eq!(data.unix_mode(), Some(ffi::S_IFLNK | 0o777));

        data.system = System::Unknown;
        assert_eq!(data.unix_mode(), Some(ffi::S_IFLNK | 0o777));

        data.external_attributes = 0x10; // DOS directory bit
        data.system = System::Dos;
        assert_eq!(data.unix_mode().unwrap() & 0o170000, ffi::S_IFDIR);
    }

    #[test]
    fn sanitize() {
        use super::{CompressionMethod, System, ZipFileData};
        use std::{path::PathBuf, sync::OnceLock};

        let file_name = "/path/../../../../etc/./passwd\0/etc/shadow".to_string();
        let data = ZipFileData {
            system: System::Dos,
            version_made_by: 0,
            flags: 0,
            compression_method: CompressionMethod::Stored,
            last_modified_time: None,
            crc32: 0,
            compressed_size: 0,
            uncompressed_size: 0,
            file_comment: String::with_capacity(0).into_boxed_str(),
            header_start: 0,
            extra_data_start: None,
            data_start: OnceLock::new(),
            central_header_start: 0,
            external_attributes: 0,
            large_file: false,
            aes_mode: None,
            ..ZipFileData::default()
        };
        assert_eq!(
            data.file_name_sanitized(&file_name),
            PathBuf::from("path/etc/passwd")
        );
    }
}
