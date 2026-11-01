//! Types that specify what is contained in a ZIP.

use crate::datetime::DateTime;
use crate::extra_fields::ExtraFields;
use crate::format::aes::{AesMode, AesVendorVersion};
use crate::format::blocks::ZipLocalEntryBlock;
use crate::format::data_descriptor::{Zip64DataDescriptorBlock, ZipDataDescriptorBlock};
use crate::format::ffi;
use crate::format::flags::ZipFileFlags;
use crate::format::flags::ZipFlags;
use crate::format::functions::{
    get_flags, get_unix_mode, get_version_needed, is_dir, needs_language_encoding,
};
use crate::format::system::System;
use crate::format::{ZIP64_BYTES_THR, ZIP64_BYTES_THR_U32};
use crate::read::{HeaderConsistencyMetadata, HeaderDataDescriptorMetadata};
use crate::result::{ZipError, ZipResult};
use crate::write::FileOptionExtension;
use crate::write::FileOptions;
use crate::{CompressionMethod, ExtraField};

use std::borrow::Cow;
use std::sync::OnceLock;

pub struct ZipRawValues {
    pub crc32: u32,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    /// The copied entry's own language encoding flag, for a copy that keeps the
    /// entry's name bytes as stored. `None` derives the flag from the name.
    pub language_encoding: Option<bool>,
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
    #[doc(hidden)]
    pub fn from_central_directory(
        system: System,
        version_made_by: u8,
        compression_method: CompressionMethod,
        last_modified_time: Option<DateTime>,
        crc32: u32,
        compressed_size: u64,
        uncompressed_size: u64,
        flags: ZipFileFlags,
        file_comment: Box<str>,
        header_start: u64,
        central_header_start: u64,
        external_attributes: u32,
        extra_fields: ExtraFields,
    ) -> Self {
        Self {
            system,
            version_made_by,
            flags,
            compression_method,
            last_modified_time,
            crc32,
            compressed_size,
            uncompressed_size,
            file_comment,
            header_start,
            central_header_start,
            data_start: OnceLock::new(),
            external_attributes,
            large_file: false,
            extra_fields,
        }
    }

    #[inline]
    pub fn name<'a>(&self, file_name_raw: &'a [u8]) -> ZipResult<Cow<'a, str>> {
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
    pub fn is_encrypted(&self) -> bool {
        self.flags.is_encrypted()
    }

    /// Check if the data descriptor flag is set
    #[inline]
    pub fn is_using_data_descriptor(&self) -> bool {
        self.flags.is_using_data_descriptor()
    }

    /// Get the starting offset of the data of the compressed file
    pub fn consistency_metadata<'a>(
        &'a self,
        name: &'a [u8],
        is_local: bool,
    ) -> HeaderConsistencyMetadata<&'a [u8]> {
        let is_using_data_descriptor = self.is_using_data_descriptor();
        let (upath, upath_crc32) = crate::read::get_upath(&self.extra_fields);
        // Only the flag bits that change how the data is decoded.
        let decoder_flags = if self.compression_method == CompressionMethod::IMPLODE {
            self.flags.as_u16()
                & (ZipFlags::CompressionSetting.as_u16() | ZipFlags::CompressionSetting2.as_u16())
        } else {
            0
        };
        HeaderConsistencyMetadata {
            name,
            upath,
            upath_crc32,
            compression_method: self.compression_method.serialize_to_u16(),
            is_encrypted: self.is_encrypted(),
            is_using_data_descriptor,
            decoder_flags,
            last_modified_time: self.last_modified_time,
            data: HeaderDataDescriptorMetadata {
                crc32: self.crc32,
                compressed_size: self.compressed_size,
                uncompressed_size: self.uncompressed_size,
                is_pending_data_descriptor: is_local && is_using_data_descriptor,
            },
            #[cfg(feature = "aes-crypto")]
            aes: self.extra_fields.inner.iter().rev().find_map(|f| match f {
                ExtraField::AeXEncryption(aes) => Some(*aes),
                _ => None,
            }),
        }
    }

    /// Validates that the local file header matches this central directory entry.
    /// Check if the file is a directory based on the file name.
    pub fn is_dir(&self, file_name: &[u8]) -> bool {
        is_dir(file_name)
    }

    /// Check if the file is a symbolic link.
    pub fn is_symlink(&self) -> bool {
        self.unix_mode()
            .is_some_and(|mode| mode & crate::format::ffi::S_IFLNK == crate::format::ffi::S_IFLNK)
    }

    /// Returns the extern compression - if AES is used, returns AES
    pub fn extern_compression(&self) -> u16 {
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
    pub const fn unix_mode(&self) -> Option<u32> {
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

    pub fn initialize_local_block<T: FileOptionExtension>(
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
            let mut external_attributes = permissions << 16;
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

    pub fn from_local_block(
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

    /// The general purpose flags to write for this entry under `file_name_raw`. The language
    /// encoding bit is as stored, except that a non-ASCII comment, which is always written as
    /// UTF-8, sets it when the name bytes are valid UTF-8 too. Such a comment can come from an
    /// Info-ZIP Unicode Comment extra field on an unflagged entry; written unflagged, a reader
    /// would decode it as CP437 and it would grow on every reopen.
    pub fn flags(&self, file_name_raw: &[u8]) -> u16 {
        let mut flags = self.flags;
        if !self.file_comment.is_ascii() && core::str::from_utf8(file_name_raw).is_ok() {
            flags.set(ZipFlags::LanguageEncoding, true);
        }
        get_flags(flags)
    }

    /// Drops any Info-ZIP Unicode Path extra field that a reader would apply to `file_name_raw`
    /// (its CRC-32 matches) but that names something else. A copy under a new name keeps the
    /// source's extra fields, and such a field would otherwise rename the copy on read-back
    /// (APPNOTE 4.6.9), e.g. back to the source's name.
    pub fn drop_unicode_path_renaming(&mut self, file_name_raw: &[u8]) {
        const UNICODE_PATH: u16 = crate::format::extra_fields::ExtraFieldId::UnicodePath.as_u16();
        let crc = crc32fast::hash(file_name_raw);
        self.extra_fields.inner.retain(|field| match field {
            ExtraField::UnicodePath(unicode) => {
                unicode.crc32 != crc || *unicode.content == *file_name_raw
            }
            ExtraField::Custom(custom) if custom.header_id == UNICODE_PATH => {
                custom.data.len() < 5
                    || custom.data[1..5] != crc.to_le_bytes()
                    || custom.data[5..] == *file_name_raw
            }
            _ => true,
        });
    }

    /// Sets the language encoding flag for this entry written under a new name.
    pub fn set_language_encoding_for(&mut self, file_name_raw: &[u8]) {
        let on = needs_language_encoding(file_name_raw, &self.file_comment);
        self.flags.set(ZipFlags::LanguageEncoding, on);
    }

    pub fn clamp_size_field(&self, field: u64) -> Result<u32, std::io::Error> {
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

    pub fn write_data_descriptor<W: std::io::Write>(
        &self,
        writer: &mut W,
        auto_large_file: bool,
    ) -> Result<(), ZipError> {
        if self.large_file {
            self.zip64_data_descriptor_block().write(writer)?;
            return Ok(());
        }
        if self.compressed_size >= ZIP64_BYTES_THR || self.uncompressed_size >= ZIP64_BYTES_THR {
            if auto_large_file {
                self.zip64_data_descriptor_block().write(writer)?;
                return Ok(());
            }
            return Err(ZipError::Io(std::io::Error::other(
                "Large file option has not been set - use .large_file(true) in options",
            )));
        }
        self.data_descriptor_block().write(writer)?;
        Ok(())
    }

    pub fn data_descriptor_block(&self) -> ZipDataDescriptorBlock {
        ZipDataDescriptorBlock {
            crc32: self.crc32,
            compressed_size: self.compressed_size as u32,
            uncompressed_size: self.uncompressed_size as u32,
        }
    }

    pub fn zip64_data_descriptor_block(&self) -> Zip64DataDescriptorBlock {
        Zip64DataDescriptorBlock {
            crc32: self.crc32,
            compressed_size: self.compressed_size,
            uncompressed_size: self.uncompressed_size,
        }
    }
}
