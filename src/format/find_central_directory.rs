//! Find central directory function

use crate::format::blocks::{
    CentralDirectoryEndInfo, DataAndPosition, Zip32CentralDirectoryEnd, Zip64CDELocatorBlock,
    Zip64CentralDirectoryEnd, Zip64CentralDirectoryEndLocator, ZipCentralEntryBlock,
};
use crate::format::magic::Magic;
use crate::read::ArchiveOffset;
use crate::read::magic_finder::{Backwards, Forward, MagicFinder};
use crate::result::{ZipResult, invalid};

use core::mem;
use std::io::{Read, Seek, SeekFrom};

const MAGIC_LENGTH: usize = mem::size_of::<Magic>();

/// Tracks forward-scanned ranges of the file for a particular magic signature, caching
/// found positions so that repeated overlapping searches run in O(log n) instead of O(n).
///
/// Invariant: `positions` is sorted and all positions are within `[0, scanned_up_to)`.
/// There are no occurrences of the signature in `[0, scanned_up_to)` other than those in
/// `positions`.
#[derive(Debug)]
pub(crate) struct MagicPositionCache {
    /// Byte positions where the magic was found, kept sorted.
    positions: Vec<u64>,
    /// We have scanned `[0, scanned_up_to)` completely. Any position ≥ scanned_up_to
    /// is unknown.
    scanned_up_to: u64,
    /// The magic bytes to search for.
    magic_bytes: [u8; MAGIC_LENGTH],
}

impl MagicPositionCache {
    pub(crate) fn new(magic_bytes: [u8; MAGIC_LENGTH]) -> Self {
        Self {
            positions: Vec::new(),
            scanned_up_to: 0,
            magic_bytes,
        }
    }

    /// Ensure we have scanned `[0, up_to)` for the magic signature.
    /// Any new occurrences found are added to `positions`.
    fn ensure_scanned_up_to<R: Read + Seek + ?Sized>(
        &mut self,
        reader: &mut R,
        up_to: u64,
    ) -> ZipResult<()> {
        if up_to <= self.scanned_up_to {
            return Ok(());
        }
        let mut finder =
            MagicFinder::<Forward<'_>>::new(&self.magic_bytes, self.scanned_up_to, up_to);
        while let Some(pos) = finder.next(reader)? {
            self.positions.push(pos);
        }
        self.scanned_up_to = up_to;
        Ok(())
    }

    /// Find the first occurrence of the magic at or after `start` and before `end`.
    /// Ensures scanning covers `[0, end)` before answering.
    pub(crate) fn first_in_range<R: Read + Seek + ?Sized>(
        &mut self,
        reader: &mut R,
        start: u64,
        end: u64,
    ) -> ZipResult<Option<u64>> {
        self.ensure_scanned_up_to(reader, end)?;
        // Binary search for the first position >= start
        let idx = self.positions.partition_point(|&p| p < start);
        if idx < self.positions.len() && self.positions[idx] < end {
            Ok(Some(self.positions[idx]))
        } else {
            Ok(None)
        }
    }
}

/// Finds the EOCD and possibly the EOCD64 block and determines the archive offset.
///
/// In the best case scenario (no prepended junk), this function will not backtrack
/// in the reader.
pub(crate) fn find_central_directory_end<R: Read + Seek + ?Sized>(
    reader: &mut R,
    archive_offset: ArchiveOffset,
    file_len: u64,
    eocd_finder: &mut MagicFinder<Backwards<'static>>,
    cdfh_cache: &mut MagicPositionCache,
    eocd64_cache: &mut MagicPositionCache,
) -> ZipResult<CentralDirectoryEndInfo> {
    const EOCD_FIXED_SIZE: u64 = 22;
    const CDFH_SIG_BYTES: [u8; mem::size_of::<Magic>()] =
        Magic::CENTRAL_DIRECTORY_HEADER_SIGNATURE.to_le_bytes();

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

                reader.seek(SeekFrom::Start(locator64_offset))?;
                let locator64 = Zip64CentralDirectoryEndLocator::parse(reader);
                Ok((locator64_offset, locator64?))
            }

            try_read_eocd64_locator(reader, eocd_offset).ok()
        } else {
            None
        };

        let Some((locator64_offset, locator64)) = zip64_metadata else {
            // Branch out for zip32
            let cd_offset_from_eocd = u64::from(eocd.central_directory_offset);

            // If the archive is empty, there is nothing more to be checked, the archive is correct.
            if eocd.number_of_files == 0 {
                // Consistency check: an empty CD ends where the EOCD starts, so its offset cannot
                // be past the EOCD. Otherwise `ZipWriter::new_append` writes the next entry at
                // that offset, growing the output to the declared size.
                if cd_offset_from_eocd > eocd_offset {
                    parsing_error = Some(invalid!("Invalid CDFH offset in EOCD"));
                    continue;
                }
                return Ok(CentralDirectoryEndInfo {
                    eocd: (eocd, eocd_offset).into(),
                    eocd64: None,
                    archive_offset: eocd_offset.saturating_sub(cd_offset_from_eocd),
                });
            }

            // Consistency check: the CD relative offset cannot be after the EOCD
            if cd_offset_from_eocd >= eocd_offset {
                parsing_error = Some(invalid!("Invalid CDFH offset in EOCD"));
                continue;
            }

            // Use the CDFH cache to find the first central directory file header.
            // The CDFH must be at or after cd_offset_from_eocd (since prepended junk
            // can only shift it forward) and before eocd_offset.
            //
            // For the optimistic case (ArchiveOffset::Known), try the expected position first
            // before falling back to the cache scan.
            let cd_result = match archive_offset {
                ArchiveOffset::Known(n) => {
                    let guess = cd_offset_from_eocd.saturating_add(n).min(eocd_offset);
                    if guess + MAGIC_LENGTH as u64 <= eocd_offset {
                        // Quick check at the guessed position
                        reader.seek(SeekFrom::Start(guess))?;
                        let mut buf = [0u8; MAGIC_LENGTH];
                        match reader.read_exact(&mut buf) {
                            Ok(()) if buf == CDFH_SIG_BYTES => Some(guess),
                            _ => {
                                // Known offset didn't work — this is an error
                                // (don't fall back to scanning when offset is known)
                                None
                            }
                        }
                    } else {
                        None
                    }
                }
                _ => {
                    // Use cache to find first CDFH in [cd_offset_from_eocd, eocd_offset)
                    cdfh_cache.first_in_range(reader, cd_offset_from_eocd, eocd_offset)?
                }
            };

            // Consistency check: find the first CDFH
            if let Some(cd_offset) = cd_result {
                // The first CDFH will define the archive offset
                let archive_offset = cd_offset - cd_offset_from_eocd;

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
            disk_with_central_directory: u32,
            expected_length: u64,
        ) -> ZipResult<Zip64CentralDirectoryEnd> {
            let z64 = Zip64CentralDirectoryEnd::parse(reader, expected_length)?;

            // Consistency check: EOCD64 locator should agree with the EOCD64
            if z64.disk_with_central_directory != disk_with_central_directory {
                return Err(invalid!("Invalid EOCD64: inconsistency with Locator data"));
            }

            // Consistency check: the EOCD64 must have the expected length
            if z64.record_size + Zip64CentralDirectoryEnd::RECORD_OVERHEAD != expected_length {
                return Err(invalid!("Invalid EOCD64: inconsistent length"));
            }

            Ok(z64)
        }

        // Use the EOCD64 cache to find the EOCD64 signature, trying the optimistic
        // guess first when the archive offset is known.
        let search_start = locator64.end_of_central_directory_offset;
        let search_end = locator64_offset;

        // Try optimistic guess first for known archive offset
        let guess_result = match archive_offset {
            ArchiveOffset::Known(n) => {
                let guess = search_start.saturating_add(n).min(search_end);
                if guess + MAGIC_LENGTH as u64 <= search_end {
                    reader.seek(SeekFrom::Start(guess))?;
                    let mut buf = [0u8; MAGIC_LENGTH];
                    match reader.read_exact(&mut buf) {
                        Ok(()) if buf == eocd64_cache.magic_bytes => Some(guess),
                        _ => None, // Known offset didn't work — don't fall back
                    }
                } else {
                    None
                }
            }
            _ => None,
        };

        // Collect candidate EOCD64 positions
        let mut local_error = None;

        // Helper closure to try a single EOCD64 candidate
        fn try_candidate<R: Read + Seek + ?Sized>(
            reader: &mut R,
            eocd64_offset: u64,
            locator64: Zip64CentralDirectoryEndLocator,
            locator64_offset: u64,
        ) -> ZipResult<Option<(DataAndPosition<Zip64CentralDirectoryEnd>, u64)>> {
            let archive_offset = eocd64_offset - locator64.end_of_central_directory_offset;
            reader.seek(SeekFrom::Start(eocd64_offset))?;

            match try_read_eocd64(
                reader,
                locator64.disk_with_central_directory,
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
                        return Ok(None);
                    }

                    Ok(Some(((eocd64, eocd64_offset).into(), archive_offset)))
                }
                Err(_e) => Ok(None),
            }
        }

        // Try the optimistic guess first
        if let Some(guess_pos) = guess_result {
            if let Some((eocd64, archive_offset)) = try_candidate(reader, guess_pos, locator64, locator64_offset)? {
                return Ok(CentralDirectoryEndInfo {
                    eocd: (eocd, guess_pos).into(),
                    eocd64: Some(eocd64),
                    archive_offset,
                });
            }
            // Known offset didn't produce valid EOCD64, don't scan further
            if matches!(archive_offset, ArchiveOffset::Known(_)) {
                parsing_error = Some(invalid!("Could not find EOCD64"));
                continue;
            }
        }

        // Fall back to cache-based search for all EOCD64 positions in range
        if !matches!(archive_offset, ArchiveOffset::Known(_)) {
            eocd64_cache.ensure_scanned_up_to(reader, search_end)?;
            let start_idx = eocd64_cache
                .positions
                .partition_point(|&p| p < search_start);
            let end_idx = eocd64_cache.positions.partition_point(|&p| p < search_end);

            for &eocd64_offset in &eocd64_cache.positions[start_idx..end_idx] {
                match try_candidate(reader, eocd64_offset, locator64, locator64_offset) {
                    Ok(Some((eocd64, archive_offset))) => {
                        return Ok(CentralDirectoryEndInfo {
                            eocd: (eocd, eocd64_offset).into(),
                            eocd64: Some(eocd64),
                            archive_offset,
                        });
                    }
                    Ok(None) => {
                        local_error =
                            Some(invalid!("Invalid EOCD64: inconsistent number of files"));
                    }
                    Err(e) => {
                        local_error = Some(e);
                    }
                }
            }
        }

        parsing_error = local_error.or(Some(invalid!("Could not find EOCD64")));
    }

    Err(parsing_error.unwrap_or(invalid!("Could not find EOCD")))
}
