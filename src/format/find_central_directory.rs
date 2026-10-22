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
/// Probe whether the magic occurs at `pos` by seeking and reading exactly `MAGIC_LENGTH` bytes.
/// Does not perform full-file scanning.
fn probe_magic_at<R: Read + Seek + ?Sized>(
    reader: &mut R,
    pos: u64,
    magic: &[u8; MAGIC_LENGTH],
) -> ZipResult<bool> {
    reader.seek(SeekFrom::Start(pos))?;
    let mut buf = [0u8; MAGIC_LENGTH];
    match reader.read_exact(&mut buf) {
        Ok(()) => Ok(buf == *magic),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Tracks scanned ranges of the file for a particular magic signature, caching
/// found positions so that repeated overlapping searches run in O(log n) instead of O(n).
///
/// Invariant: `positions` is sorted and contains all occurrences of `magic_bytes`
/// within the union of intervals in `scanned_ranges`.
#[derive(Debug)]
pub(crate) struct MagicPositionCache {
    /// Byte positions where the magic was found, kept sorted.
    positions: Vec<u64>,
    /// Disjoint, sorted ranges `[start, end)` that have been scanned.
    scanned_ranges: Vec<(u64, u64)>,
    /// The magic bytes to search for.
    magic_bytes: [u8; MAGIC_LENGTH],
}

impl MagicPositionCache {
    pub(crate) fn new(magic_bytes: [u8; MAGIC_LENGTH]) -> Self {
        Self {
            positions: Vec::new(),
            scanned_ranges: Vec::new(),
            magic_bytes,
        }
    }

    /// Ensure that `[start, end)` has been scanned for the magic signature.
    /// Only scans sub-ranges that have not already been covered by prior scans,
    /// avoiding full-file scans across multi-gigabyte payloads.
    pub(crate) fn ensure_scanned_range<R: Read + Seek + ?Sized>(
        &mut self,
        reader: &mut R,
        start: u64,
        end: u64,
    ) -> ZipResult<()> {
        if start >= end {
            return Ok(());
        }

        let mut uncovered = Vec::new();
        let mut cur = start;
        for &(r_start, r_end) in &self.scanned_ranges {
            if cur >= end {
                break;
            }
            if cur < r_start {
                let scan_end = end.min(r_start);
                uncovered.push((cur, scan_end));
                cur = scan_end;
            }
            if cur < r_end {
                cur = r_end;
            }
        }
        if cur < end {
            uncovered.push((cur, end));
        }

        for (u_start, u_end) in uncovered {
            self.scan_uncovered(reader, u_start, u_end)?;
        }

        self.merge_scanned_range(start, end);
        Ok(())
    }

    fn scan_uncovered<R: Read + Seek + ?Sized>(
        &mut self,
        reader: &mut R,
        start: u64,
        end: u64,
    ) -> ZipResult<()> {
        // Overlap backwards by MAGIC_LENGTH - 1 to catch any signature straddling the scan boundary
        let magic_overlap = MAGIC_LENGTH.saturating_sub(1) as u64;
        let scan_start = start.saturating_sub(magic_overlap);
        let mut finder = MagicFinder::<Forward<'_>>::new(&self.magic_bytes, scan_start, end);
        while let Some(pos) = finder.next(reader)? {
            if let Err(idx) = self.positions.binary_search(&pos) {
                self.positions.insert(idx, pos);
            }
        }
        Ok(())
    }

    fn merge_scanned_range(&mut self, start: u64, end: u64) {
        self.scanned_ranges.push((start, end));
        self.scanned_ranges.sort_unstable_by_key(|&(s, _)| s);
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(self.scanned_ranges.len());
        for (s, e) in self.scanned_ranges.drain(..) {
            if let Some(last) = merged.last_mut()
                && s <= last.1
            {
                last.1 = last.1.max(e);
                continue;
            }
            merged.push((s, e));
        }
        self.scanned_ranges = merged;
    }

    /// Find the first occurrence of the magic at or after `start` and before `end`.
    /// Ensures scanning covers only `[start, end)` before answering.
    pub(crate) fn first_in_range<R: Read + Seek + ?Sized>(
        &mut self,
        reader: &mut R,
        start: u64,
        end: u64,
    ) -> ZipResult<Option<u64>> {
        self.ensure_scanned_range(reader, start, end)?;
        let idx = self.positions.partition_point(|&p| p < start);
        if idx < self.positions.len() && self.positions[idx] < end {
            Ok(Some(self.positions[idx]))
        } else {
            Ok(None)
        }
    }

    /// Whether the magic occurs at `pos`.
    /// Performs an O(1) probe at `pos` without scanning the entire file.
    pub(crate) fn contains<R: Read + Seek + ?Sized>(
        &mut self,
        reader: &mut R,
        pos: u64,
        _end: u64,
    ) -> ZipResult<bool> {
        if self.positions.binary_search(&pos).is_ok() {
            return Ok(true);
        }
        let found = probe_magic_at(reader, pos, &self.magic_bytes)?;
        if found {
            let idx = self.positions.partition_point(|&p| p < pos);
            self.positions.insert(idx, pos);
        }
        Ok(found)
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
            // Consistency check: multi-disk archives are not supported
            if eocd.disk_number != 0
                || eocd.disk_with_central_directory != 0
                || eocd.number_of_files_on_this_disk != eocd.number_of_files
            {
                parsing_error = Some(invalid!("Multi-disk ZIP files are not supported"));
                continue;
            }

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
                    if guess + MAGIC_LENGTH as u64 <= eocd_offset
                        && probe_magic_at(reader, guess, &CDFH_SIG_BYTES)?
                    {
                        Some(guess)
                    } else {
                        None
                    }
                }
                _ => {
                    // Prefer the CD start implied by the CD size, as other readers do.
                    let abutting = eocd_offset
                        .checked_sub(u64::from(eocd.central_directory_size))
                        .filter(|&start| start >= cd_offset_from_eocd);
                    match abutting {
                        Some(start) if cdfh_cache.contains(reader, start, eocd_offset)? => {
                            Some(start)
                        }
                        // Otherwise use cache to find first CDFH in [cd_offset_from_eocd, eocd_offset)
                        _ => cdfh_cache.first_in_range(reader, cd_offset_from_eocd, eocd_offset)?,
                    }
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

            // Consistency check: multi-disk archives are not supported
            if z64.disk_number != 0 || z64.number_of_files_on_this_disk != z64.number_of_files {
                return Err(invalid!("Multi-disk ZIP files are not supported"));
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

        // Try optimistic guess first for known archive offset or un-prefixed archives
        let guess_result = match archive_offset {
            ArchiveOffset::Known(n) => {
                let guess = search_start.saturating_add(n).min(search_end);
                if guess + MAGIC_LENGTH as u64 <= search_end
                    && probe_magic_at(reader, guess, &eocd64_cache.magic_bytes)?
                {
                    Some(guess)
                } else {
                    None
                }
            }
            ArchiveOffset::Detect => {
                // In standard un-prefixed archives, EOCD64 is exactly at search_start.
                // Probing this first avoids a forward scan from byte 0 across the entire archive.
                if search_start + MAGIC_LENGTH as u64 <= search_end
                    && probe_magic_at(reader, search_start, &eocd64_cache.magic_bytes)?
                {
                    Some(search_start)
                } else {
                    None
                }
            }
        };

        // Collect candidate EOCD64 positions
        let mut local_error = None;

        // Helper closure to try a single EOCD64 candidate
        fn try_candidate<R: Read + Seek + ?Sized>(
            reader: &mut R,
            eocd_offset: u64,
            eocd64_offset: u64,
            disk_with_central_directory: u32,
            locator64_offset: u64,
        ) -> ZipResult<Option<(DataAndPosition<Zip64CentralDirectoryEnd>, u64)>> {
            let Some(archive_offset) = eocd64_offset.checked_sub(eocd_offset) else {
                return Ok(None);
            };
            reader.seek(SeekFrom::Start(eocd64_offset))?;

            let eocd64 = try_read_eocd64(
                reader,
                disk_with_central_directory,
                locator64_offset.saturating_sub(eocd64_offset),
            )?;

            if eocd64_offset
                < eocd64
                    .number_of_files
                    .saturating_mul(
                        (mem::size_of::<Magic>() + mem::size_of::<ZipCentralEntryBlock>()) as u64,
                    )
                    .saturating_add(eocd64.central_directory_offset)
            {
                return Ok(None);
            }

            Ok(Some(((eocd64, eocd64_offset).into(), archive_offset)))
        }

        // Try the optimistic guess first
        if let Some(guess_pos) = guess_result {
            match try_candidate(
                reader,
                locator64.end_of_central_directory_offset,
                guess_pos,
                locator64.disk_with_central_directory,
                locator64_offset,
            ) {
                Ok(Some((eocd64, archive_offset))) => {
                    return Ok(CentralDirectoryEndInfo {
                        eocd: (eocd, guess_pos).into(),
                        eocd64: Some(eocd64),
                        archive_offset,
                    });
                }
                Ok(None) => {}
                Err(e) => {
                    local_error = Some(e);
                }
            }
            // Known offset didn't produce valid EOCD64, don't scan further
            if matches!(archive_offset, ArchiveOffset::Known(_)) {
                parsing_error = local_error.or(Some(invalid!("Could not find EOCD64")));
                continue;
            }
        }

        // Fall back to cache-based search for all EOCD64 positions in range
        if !matches!(archive_offset, ArchiveOffset::Known(_)) {
            eocd64_cache.ensure_scanned_range(reader, search_start, search_end)?;
            let start_idx = eocd64_cache
                .positions
                .partition_point(|&p| p < search_start);
            let end_idx = eocd64_cache.positions.partition_point(|&p| p < search_end);

            for &eocd64_offset in &eocd64_cache.positions[start_idx..end_idx] {
                match try_candidate(
                    reader,
                    locator64.end_of_central_directory_offset,
                    eocd64_offset,
                    locator64.disk_with_central_directory,
                    locator64_offset,
                ) {
                    Ok(Some((eocd64, archive_offset))) => {
                        return Ok(CentralDirectoryEndInfo {
                            eocd: (eocd, eocd64_offset).into(),
                            eocd64: Some(eocd64),
                            archive_offset,
                        });
                    }
                    Ok(None) => {
                        if local_error.is_none() {
                            local_error =
                                Some(invalid!("Invalid EOCD64: inconsistent number of files"));
                        }
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
