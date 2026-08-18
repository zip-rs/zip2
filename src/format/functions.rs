//! Zip utils functions

use crate::CompressionMethod;
use crate::format::aes::{AesMode, AesVendorVersion};
use crate::format::flags::{ZipFileFlags, ZipFlags};
use crate::format::system::System;
use crate::format::{DEFAULT_VERSION, MIN_VERSION, ffi};

#[inline]
pub(crate) fn is_dir(filename: &[u8]) -> bool {
    matches!(filename.last(), Some(b'/') | Some(b'\\'))
}

#[inline]
pub(crate) const fn get_unix_mode(system: System, external_attributes: u32) -> Option<u32> {
    if external_attributes == 0 {
        return None;
    }
    let unix_mode = external_attributes >> 16;
    match system {
        System::Unix => Some(unix_mode),
        System::Dos => {
            // For MS-DOS, the low order byte is the MS-DOS directory attribute byte.
            let dos_attributes = (external_attributes & 0xFF) as u8;
            // Interpret MS-DOS directory bit
            let mut mode = if (dos_attributes & 0x10) != 0 {
                ffi::S_IFDIR | 0o0775
            } else {
                ffi::S_IFREG | 0o0664
            };
            // Interpret MS-DOS read-only bit
            if (dos_attributes & 0x01) != 0 {
                // strip write permissions for read-only
                mode &= !0o222;
            }
            Some(mode)
        }
        _ => {
            if unix_mode != 0 {
                // If the high 16 bits are non-zero, they probably contain Unix permissions.
                // This happens for archives created on Windows by this crate or other tools,
                // and is the only way to identify symlinks in such archives.
                return Some(unix_mode);
            }
            None
        }
    }
}

#[inline]
pub(crate) fn get_version_needed(
    compression_method: CompressionMethod,
    aes_settings: Option<(AesMode, AesVendorVersion)>,
    is_encrypted: bool,
    large_file: bool,
    unix_mode: Option<u32>,
) -> u16 {
    let compression_version: u16 = match compression_method {
        CompressionMethod::Stored => u16::from(MIN_VERSION),
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
    let crypto_version: u16 = if aes_settings.is_some() {
        51
    } else if is_encrypted {
        20
    } else {
        10
    };
    let misc_feature_version: u16 = if large_file {
        45
    } else if unix_mode.is_some_and(|mode| mode & ffi::S_IFDIR == ffi::S_IFDIR) {
        // file is directory
        20
    } else {
        10
    };
    compression_version
        .max(crypto_version)
        .max(misc_feature_version)
}

pub(crate) fn get_flags(flags: ZipFileFlags, file_name_raw: &[u8], file_comment: &str) -> u16 {
    let is_utf8 = core::str::from_utf8(file_name_raw).is_ok();
    let is_ascii = file_name_raw.is_ascii() && file_comment.is_ascii();
    let utf8_bit: u16 = if is_utf8 && !is_ascii {
        ZipFlags::LanguageEncoding.as_u16()
    } else {
        0
    };

    let using_data_descriptor_bit = if flags.is_using_data_descriptor() {
        ZipFlags::UsingDataDescriptor.as_u16()
    } else {
        0
    };

    let encrypted_bit: u16 = if flags.is_encrypted() { 1u16 << 0 } else { 0 };

    utf8_bit | using_data_descriptor_bit | encrypted_bit
}

#[cfg(test)]
mod tests {

    #[test]
    fn unix_mode_robustness() {
        use crate::format::ffi;
        use crate::format::functions::get_unix_mode;
        use crate::format::system::System;
        // Also, if we use the `unix_permissions()` in the `FileOptions`
        // The ZipFileData will be forced to be System::Unix if we use a symlink

        // DOS/FAT filesystems have no concept of symlinks
        // In our case, we handle that by defaulting to Unix
        let system = System::Dos;
        let external_attributes = (ffi::S_IFLNK | 0o777) << 16;
        let unix_mode = get_unix_mode(system, external_attributes);
        assert_eq!(unix_mode, Some(ffi::S_IFREG | 0o664));

        let system = System::Unknown;
        let external_attributes = (ffi::S_IFLNK | 0o777) << 16;
        let unix_mode = get_unix_mode(system, external_attributes);
        assert_eq!(unix_mode, Some(ffi::S_IFLNK | 0o777));

        let system = System::Dos;
        let external_attributes = 0x10; // DOS directory bit
        let unix_mode = get_unix_mode(system, external_attributes);
        assert_eq!(unix_mode.unwrap() & 0o170000, ffi::S_IFDIR);
    }
}
