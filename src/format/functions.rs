//! Zip utils functions

use crate::CompressionMethod;
use crate::format::aes::{AesMode, AesVendorVersion};
use crate::format::blocks::{
    CentralDirectoryEndInfo, Zip32CentralDirectoryEnd, Zip64CDELocatorBlock,
    Zip64CentralDirectoryEnd, Zip64CentralDirectoryEndLocator, ZipCentralEntryBlock,
};
use crate::format::flags::System;
use crate::format::magic::Magic;
use crate::format::{DEFAULT_VERSION, MIN_VERSION, ffi};
use crate::read::ArchiveOffset;
use crate::read::magic_finder::{Backwards, Forward, MagicFinder, OptimisticMagicFinder};
use crate::result::{ZipResult, invalid};

use core::mem;
use std::io::{self, Read, Seek};

/// Finds the EOCD and possibly the EOCD64 block and determines the archive offset.
///
/// In the best case scenario (no prepended junk), this function will not backtrack
/// in the reader.
pub(crate) fn find_central_directory<R: Read + Seek + ?Sized>(
    reader: &mut R,
    archive_offset: ArchiveOffset,
    end_exclusive: u64,
    file_len: u64,
) -> ZipResult<CentralDirectoryEndInfo> {
    const EOCD_SIG_BYTES: [u8; mem::size_of::<Magic>()] =
        Magic::CENTRAL_DIRECTORY_END_SIGNATURE.to_le_bytes();

    const EOCD64_SIG_BYTES: [u8; mem::size_of::<Magic>()] =
        Magic::ZIP64_CENTRAL_DIRECTORY_END_SIGNATURE.to_le_bytes();

    const CDFH_SIG_BYTES: [u8; mem::size_of::<Magic>()] =
        Magic::CENTRAL_DIRECTORY_HEADER_SIGNATURE.to_le_bytes();

    const EOCD_FIXED_SIZE: u64 = 22;

    // Instantiate the mandatory finder
    let mut eocd_finder = MagicFinder::<Backwards<'static>>::new(&EOCD_SIG_BYTES, 0, end_exclusive);
    let mut subfinder: Option<OptimisticMagicFinder<Forward<'static>>> = None;

    // Keep the last errors for cases of improper EOCD instances.
    let mut parsing_error = None;

    while let Some(eocd_offset) = eocd_finder.next(reader)? {
        // Attempt to parse the EOCD block
        let eocd = match Zip32CentralDirectoryEnd::parse(reader) {
            Ok(eocd) => eocd,
            Err(e) => {
                if parsing_error.is_none() {
                    parsing_error = Some(e);
                }
                continue;
            }
        };

        // ! Relaxed (inequality) due to garbage-after-comment Python files
        // Consistency check: the EOCD comment must terminate before the end of file
        if eocd.zip_file_comment.len() as u64 + eocd_offset + EOCD_FIXED_SIZE > file_len {
            parsing_error = Some(invalid!("Invalid EOCD comment length"));
            continue;
        }

        let zip64_metadata = if eocd.may_be_zip64() {
            fn try_read_eocd64_locator(
                reader: &mut (impl Read + Seek + ?Sized),
                eocd_offset: u64,
            ) -> ZipResult<(u64, Zip64CentralDirectoryEndLocator)> {
                if eocd_offset
                    < (mem::size_of::<Magic>() + mem::size_of::<Zip64CDELocatorBlock>()) as u64
                {
                    return Err(invalid!("EOCD64 Locator does not fit in file"));
                }

                let locator64_offset = eocd_offset
                    - (mem::size_of::<Magic>() + mem::size_of::<Zip64CDELocatorBlock>()) as u64;

                reader.seek(io::SeekFrom::Start(locator64_offset))?;
                let locator64 = Zip64CentralDirectoryEndLocator::parse(reader);
                Ok((locator64_offset, locator64?))
            }

            try_read_eocd64_locator(reader, eocd_offset).ok()
        } else {
            None
        };

        let Some((locator64_offset, locator64)) = zip64_metadata else {
            // Branch out for zip32
            let relative_cd_offset = u64::from(eocd.central_directory_offset);

            // If the archive is empty, there is nothing more to be checked, the archive is correct.
            if eocd.number_of_files == 0 {
                return Ok(CentralDirectoryEndInfo {
                    eocd: (eocd, eocd_offset).into(),
                    eocd64: None,
                    archive_offset: eocd_offset.saturating_sub(relative_cd_offset),
                });
            }

            // Consistency check: the CD relative offset cannot be after the EOCD
            if relative_cd_offset >= eocd_offset {
                parsing_error = Some(invalid!("Invalid CDFH offset in EOCD"));
                continue;
            }

            // Attempt to find the first CDFH
            let subfinder = subfinder
                .get_or_insert_with(OptimisticMagicFinder::new_empty)
                .repurpose(
                    &CDFH_SIG_BYTES,
                    // The CDFH must be before the EOCD and after the relative offset,
                    // because prepended junk can only move it forward.
                    (relative_cd_offset, eocd_offset),
                    match archive_offset {
                        ArchiveOffset::Known(n) => {
                            Some((relative_cd_offset.saturating_add(n).min(eocd_offset), true))
                        }
                        _ => Some((relative_cd_offset, false)),
                    },
                );

            // Consistency check: find the first CDFH
            if let Some(cd_offset) = subfinder.next(reader)? {
                // The first CDFH will define the archive offset
                let archive_offset = cd_offset - relative_cd_offset;

                return Ok(CentralDirectoryEndInfo {
                    eocd: (eocd, eocd_offset).into(),
                    eocd64: None,
                    archive_offset,
                });
            }

            parsing_error = Some(invalid!("No CDFH found"));
            continue;
        };

        // Consistency check: the EOCD64 offset must be before EOCD64 Locator offset */
        if locator64.end_of_central_directory_offset >= locator64_offset {
            parsing_error = Some(invalid!("Invalid EOCD64 Locator CD offset"));
            continue;
        }

        if locator64.number_of_disks > 1 {
            parsing_error = Some(invalid!("Multi-disk ZIP files are not supported"));
            continue;
        }

        // This was hidden inside a function to collect errors in a single place.
        // Once try blocks are stabilized, this can go away.
        fn try_read_eocd64<R: Read + Seek + ?Sized>(
            reader: &mut R,
            locator64: &Zip64CentralDirectoryEndLocator,
            expected_length: u64,
        ) -> ZipResult<Zip64CentralDirectoryEnd> {
            let z64 = Zip64CentralDirectoryEnd::parse(reader, expected_length)?;

            // Consistency check: EOCD64 locator should agree with the EOCD64
            if z64.disk_with_central_directory != locator64.disk_with_central_directory {
                return Err(invalid!("Invalid EOCD64: inconsistency with Locator data"));
            }

            // Consistency check: the EOCD64 must have the expected length
            if z64.record_size + Zip64CentralDirectoryEnd::RECORD_OVERHEAD != expected_length {
                return Err(invalid!("Invalid EOCD64: inconsistent length"));
            }

            Ok(z64)
        }

        // Attempt to find the EOCD64 with an initial guess
        let subfinder = subfinder
            .get_or_insert_with(OptimisticMagicFinder::new_empty)
            .repurpose(
                &EOCD64_SIG_BYTES,
                (locator64.end_of_central_directory_offset, locator64_offset),
                match archive_offset {
                    ArchiveOffset::Known(n) => Some((
                        locator64
                            .end_of_central_directory_offset
                            .saturating_add(n)
                            .min(locator64_offset),
                        true,
                    )),
                    _ => Some((locator64.end_of_central_directory_offset, false)),
                },
            );

        // Consistency check: Find the EOCD64
        let mut local_error = None;
        while let Some(eocd64_offset) = subfinder.next(reader)? {
            let archive_offset = eocd64_offset - locator64.end_of_central_directory_offset;

            match try_read_eocd64(
                reader,
                &locator64,
                locator64_offset.saturating_sub(eocd64_offset),
            ) {
                Ok(eocd64) => {
                    if eocd64_offset
                        < eocd64
                            .number_of_files
                            .saturating_mul(
                                (mem::size_of::<Magic>() + mem::size_of::<ZipCentralEntryBlock>())
                                    as u64,
                            )
                            .saturating_add(eocd64.central_directory_offset)
                    {
                        local_error =
                            Some(invalid!("Invalid EOCD64: inconsistent number of files"));
                        continue;
                    }

                    return Ok(CentralDirectoryEndInfo {
                        eocd: (eocd, eocd_offset).into(),
                        eocd64: Some((eocd64, eocd64_offset).into()),
                        archive_offset,
                    });
                }
                Err(e) => {
                    local_error = Some(e);
                }
            }
        }

        parsing_error = local_error.or(Some(invalid!("Could not find EOCD64")));
    }

    Err(parsing_error.unwrap_or(invalid!("Could not find EOCD")))
}

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
