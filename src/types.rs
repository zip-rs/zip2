//! Types that specify what is contained in a ZIP.

use crate::datetime::DateTime;
use crate::extra_fields::ExtraFields;
use crate::format::aes::{AesMode, AesVendorVersion};
use crate::format::blocks::{
    FixedSizeBlock, Zip64DataDescriptorBlock, ZipDataDescriptorBlock, ZipLocalEntryBlock,
};
use crate::format::ffi;
use crate::format::flags::ZipFileFlags;
use crate::format::flags::ZipFlags;
use crate::format::functions::{get_flags, get_unix_mode, get_version_needed, is_dir};
use crate::format::magic::Magic;
use crate::format::system::System;
use crate::format::{ZIP64_BYTES_THR, ZIP64_BYTES_THR_U32};
use crate::path::{enclosed_name, file_name_sanitized};
use crate::read::readers::{SeekableTake, ZipFileReader, make_crypto_reader, make_reader};
use crate::result::{ZipError, ZipResult};
use crate::write::FileOptionExtension;
use crate::write::FileOptions;
use crate::{CompressionMethod, ZipReadOptions};

use std::borrow::Cow;
use std::ffi::OsStr;
use std::io::{Read, Seek, SeekFrom, Take};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub(crate) struct ZipRawValues {
    pub(crate) crc32: u32,
    pub(crate) compressed_size: u64,
    pub(crate) uncompressed_size: u64,
}

/// re-export
pub use crate::format::{DEFAULT_VERSION, MIN_VERSION};

/// Structure representing a ZIP file.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ZipFileData {
    /// Compatibility of the file attribute information
    pub system: System,
    /// Specification version
    pub version_made_by: u8,
    /// ZIP flags
    pub flags: ZipFileFlags,
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
    /// extra fields, see <https://libzip.org/specifications/extrafld.txt>
    pub extra_fields: ExtraFields,
}

impl ZipFileData {
    pub(crate) fn make_reader<'a, R: Read>(
        &self,
        limit_reader: Take<&'a mut R>,
        mut options: ZipReadOptions<'_>,
    ) -> ZipResult<ZipFileReader<'a, R>> {
        if options.ignore_encryption_flag {
            // Always use no password when we're ignoring the encryption flag.
            options.password = None;
        } else {
            // Require and use the password only if the file is encrypted.
            match (options.password, self.is_encrypted()) {
                (None, true) => {
                    return Err(ZipError::UnsupportedArchive(ZipError::PASSWORD_REQUIRED));
                }
                // Password supplied, but none needed! Discard.
                (Some(_), false) => options.password = None,
                _ => {}
            }
        }
        let compression_method = self.compression_method;
        let uncompressed_size = self.uncompressed_size;
        let crc32 = if options.ignore_crc {
            None
        } else {
            Some(self.crc32)
        };
        let crypto_reader = make_crypto_reader(self, limit_reader, options.password)?;
        let aes_vendor_version = self.aes_settings().map(|aes| aes.1);
        make_reader(
            compression_method,
            uncompressed_size,
            crc32,
            aes_vendor_version,
            crypto_reader,
            #[cfg(feature = "legacy-zip")]
            self.flags.as_u16(),
        )
    }

    #[inline]
    pub(crate) fn name<'a>(&self, file_name_raw: &'a [u8]) -> ZipResult<Cow<'a, str>> {
        use crate::cp437::FromCp437;
        Ok(
            if let Ok(file_name_utf8) = std::str::from_utf8(file_name_raw) {
                file_name_utf8.into()
            } else {
                file_name_raw.from_cp437().map_err(std::io::Error::other)?
            },
        )
    }

    pub fn aes_settings(&self) -> Option<(AesMode, AesVendorVersion)> {
        #[cfg(feature = "aes-crypto")]
        {
            use crate::ExtraField;
            for one_extra in &self.extra_fields.inner {
                if let ExtraField::AeXEncryption(aes) = one_extra {
                    return Some((aes.aes_mode, aes.aes_vendor_version));
                }
            }
            None
        }
        #[cfg(not(feature = "aes-crypto"))]
        {
            None
        }
    }

    /// Check if the encrypted flag is set
    #[inline]
    pub(crate) fn is_encrypted(&self) -> bool {
        self.flags.is_encrypted()
    }

    /// Check if the data descriptor flag is set
    #[inline]
    pub(crate) fn is_using_data_descriptor(&self) -> bool {
        self.flags.is_using_data_descriptor()
    }

    /// Get the starting offset of the data of the compressed file
    pub fn data_start(&self, reader: &mut (impl Read + Seek + ?Sized)) -> ZipResult<u64> {
        match self.data_start.get() {
            Some(data_start) => Ok(*data_start),
            None => {
                // Go to start of data.
                reader.seek(SeekFrom::Start(self.header_start))?;

                // Parse static-sized fields and check the magic value.
                let block = ZipLocalEntryBlock::parse(reader)?;

                // Each of these fields must be converted to u64 before adding, as the result may
                // easily overflow a u16.
                let variable_fields_len =
                    u64::from(block.file_name_length) + u64::from(block.extra_field_length);
                // Calculate the end of the local header from the fields we just parsed.
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
        }
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
    pub(crate) fn is_dir(&self, file_name: &[u8]) -> bool {
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

    /// Returns the extern compression - if AES is used, returns AES
    pub(crate) fn extern_compression(&self) -> u16 {
        #[cfg(feature = "aes-crypto")]
        {
            // without feature, aes_settings() returns None
            if self.aes_settings().is_some() {
                CompressionMethod::AES.serialize_to_u16()
            } else {
                self.compression_method.serialize_to_u16()
            }
        }
        #[cfg(not(feature = "aes-crypto"))]
        {
            self.compression_method.serialize_to_u16()
        }
    }

    /// Get unix mode for the file
    pub(crate) const fn unix_mode(&self) -> Option<u32> {
        get_unix_mode(self.system, self.external_attributes)
    }

    /// PKZIP version needed to open this file (from APPNOTE 4.4.3.2).
    pub fn version_needed(&self) -> u16 {
        get_version_needed(
            self.compression_method,
            self.aes_settings(),
            self.is_encrypted(),
            self.large_file,
            self.unix_mode(),
        )
    }

    pub(crate) fn initialize_local_block<T: FileOptionExtension>(
        file_name_raw: &[u8],
        options: &FileOptions<'_, '_, T>,
        raw_values: &ZipRawValues,
        header_start: u64,
        #[cfg_attr(not(feature = "aes-crypto"), allow(unused_mut))] mut extra_fields: ExtraFields,
    ) -> Self {
        // Figure out the underlying compression_method and aes mode when using
        // AES encryption.
        // Preserve AES method for raw copies without needing a password
        let compression_method = options.compression_method;
        match options.encrypt_with {
            #[cfg(feature = "aes-crypto")]
            Some(crate::write::options::EncryptWith::Aes {
                mode,
                vendor_version,
                ..
            }) => {
                use crate::extra_fields::AexEncryption;
                // Write AES encryption extra data.
                // For raw copies of AES entries, write the correct AES extra data immediately
                extra_fields
                    .inner
                    .push(crate::extra_fields::ExtraField::AeXEncryption(
                        AexEncryption::new(vendor_version, mode, compression_method),
                    ));
            }
            _ => {}
        }
        let permissions = options
            .permissions
            .unwrap_or(FileOptions::DEFAULT_FILE_PERMISSION);
        let mut external_attributes = permissions << 16;
        let system = if (permissions & ffi::S_IFLNK) == ffi::S_IFLNK {
            // DOS/FAT filesystems have no concept of symlinks
            // We force to System::Unix
            System::Unix
        } else if let Some(system_option) = options.system {
            // user provided
            system_option
        } else if cfg!(windows) {
            System::Dos
        } else {
            System::Unix
        };
        let external_attributes = if let Some(external_attr) = options.external_attributes {
            external_attr
        } else {
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
            external_attributes
        };
        let mut flags = ZipFileFlags(0);
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
            extra_fields,
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
            flags: ZipFileFlags(flags),
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
            extra_fields,
        };
        Ok(data)
    }

    pub(crate) fn flags(&self, file_name_raw: &[u8]) -> u16 {
        get_flags(self.flags, file_name_raw, self.file_comment.as_ref())
    }

    pub(crate) fn clamp_size_field(&self, field: u64) -> Result<u32, std::io::Error> {
        if self.large_file {
            Ok(ZIP64_BYTES_THR_U32)
        } else {
            let size: u32 = field.try_into().map_err(|_| {
                std::io::Error::other(format!(
                    "File size {field} exceeds maximum size for non-ZIP64 files"
                ))
            })?;
            Ok(size.min(ZIP64_BYTES_THR_U32 - 1))
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
        if self.compressed_size >= ZIP64_BYTES_THR || self.uncompressed_size >= ZIP64_BYTES_THR {
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
    fn sanitize() {
        use super::{CompressionMethod, System, ZipFileData};
        use std::{path::PathBuf, sync::OnceLock};

        let file_name = "/path/../../../../etc/./passwd\0/etc/shadow".to_string();
        let data = ZipFileData {
            system: System::Dos,
            compression_method: CompressionMethod::Stored,
            file_comment: String::with_capacity(0).into_boxed_str(),
            header_start: 0,
            data_start: OnceLock::new(),
            large_file: false,
            ..ZipFileData::default()
        };
        assert_eq!(
            data.file_name_sanitized(&file_name),
            PathBuf::from("path/etc/passwd")
        );
    }
}
