//! Iterable ZIP archive reader.
//!
//! [`ZipIterable`] walks the central directory one entry at a time instead of building an
//! in-memory index of the whole archive. This keeps memory usage independent of the number of
//! entries, at the cost of re-reading the central directory for every pass. It also preserves
//! entries that share a name, which [`ZipArchive`](crate::ZipArchive) cannot represent because it
//! keys entries by name.
//!
//! The reader itself must be seekable: the central directory is discovered from the end of the
//! archive, and entry contents are read by seeking back to their local headers. For a
//! non-seekable stream, use [`read_zipfile_from_stream`](crate::read::read_zipfile_from_stream)
//! instead.

use crate::format::MagicPositionCache;
use crate::format::blocks::{FixedSizeBlock, Zip32CentralDirectoryEnd, ZipCentralEntryBlock};
use crate::format::find_central_directory_end;
use crate::format::magic::Magic;
use crate::read::magic_finder::{Backwards, MagicFinder};
use crate::read::zipfile::ZipFileEntry;
use crate::read::{CentralDirectoryInfo, Config, central_header_to_zip_file_inner};
use crate::result::{ZipError, ZipResult, invalid};
use std::borrow::Cow;
use std::io::{Read, Seek, SeekFrom};
use std::mem::size_of;

/// Iterable version of [`ZipArchive`](crate::ZipArchive).
///
/// Unlike `ZipArchive`, this type does not store metadata for every entry in memory. Entries are
/// parsed from the central directory on demand, which makes it suitable for archives with a very
/// large number of entries.
///
/// Every call to [`Self::files`] starts a new pass from the beginning of the central directory.
#[derive(Debug)]
pub struct ZipIterable<R> {
    files: ZipIterableFiles<R>,
}

impl<R: Read + Seek> ZipIterable<R> {
    /// Try to create a new iterable archive from a seekable reader.
    ///
    /// The whole archive is not loaded. Only the end-of-central-directory record and, if
    /// present, the ZIP64 end record are read up front.
    pub fn try_new(mut reader: R, config: Config) -> ZipResult<Self> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        let min_eocd_position = file_len.saturating_sub(
            (size_of::<Zip32CentralDirectoryEnd>() + size_of::<Magic>() + (1 << 16)) as u64,
        );

        const EOCD_SIG_BYTES: [u8; size_of::<Magic>()] =
            Magic::CENTRAL_DIRECTORY_END_SIGNATURE.to_le_bytes();
        const EOCD64_SIG_BYTES: [u8; size_of::<Magic>()] =
            Magic::ZIP64_CENTRAL_DIRECTORY_END_SIGNATURE.to_le_bytes();
        const CDFH_SIG_BYTES: [u8; size_of::<Magic>()] =
            Magic::CENTRAL_DIRECTORY_HEADER_SIGNATURE.to_le_bytes();

        let mut eocd_finder: MagicFinder<Backwards<'static>> =
            MagicFinder::new(&EOCD_SIG_BYTES, min_eocd_position, file_len);
        let mut cdfh_cache = MagicPositionCache::new(CDFH_SIG_BYTES);
        let mut eocd64_cache = MagicPositionCache::new(EOCD64_SIG_BYTES);

        let mut last_err = None;
        let central_directory = loop {
            let cde = match find_central_directory_end(
                &mut reader,
                config.archive_offset,
                file_len,
                &mut eocd_finder,
                &mut cdfh_cache,
                &mut eocd64_cache,
            ) {
                Ok(cde) => cde,
                Err(e) => return Err(last_err.unwrap_or(e)),
            };

            match CentralDirectoryInfo::try_from(&cde) {
                Ok(info) => break info,
                Err(e) => {
                    // Other readers use a record whose CD ends at it, so don't fall back past it.
                    if cde.central_directory_abuts_end() {
                        return Err(e);
                    }
                    last_err = Some(e);
                }
            }
        };

        if central_directory.disk_number != central_directory.disk_with_central_directory {
            return Err(ZipError::UnsupportedArchive(
                "Support for multi-disk files is not implemented",
            ));
        }

        // A central directory header is at least 46 bytes, so this bounds the declared entry
        // count by the space the directory actually occupies.
        let max_entries =
            usize::try_from(central_directory.directory_size / 46).unwrap_or(usize::MAX);
        if central_directory.number_of_files > max_entries {
            return Err(invalid!(
                "Central directory declares more entries than its size allows"
            ));
        }

        Ok(Self {
            files: ZipIterableFiles::new(reader, central_directory),
        })
    }

    /// Number of entries in the central directory.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.remaining
    }

    /// Whether the central directory contains no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Start a new pass over the central directory.
    ///
    /// The returned iterator borrows the underlying reader and yields owned entries. The
    /// previous pass is discarded, even if it was only partially consumed.
    pub fn files(&mut self) -> ZipResult<&mut ZipIterableFiles<R>> {
        self.files.reset()?;
        Ok(&mut self.files)
    }

    /// Validate each entry's local file header against the central directory before yielding it.
    ///
    /// When enabled, any discrepancy (such as Zip Cloaking / Dual-Header Smuggling attacks,
    /// where an attacker provides different names or hidden files across headers) makes that
    /// entry fail with [`ZipError::InvalidArchive`] and stops the pass.
    ///
    /// Duplicate names are not treated as an inconsistency: preserving them is the reason this
    /// reader exists. Use [`ZipArchive::validate_header_consistency`] when duplicates must be
    /// rejected.
    ///
    /// [`ZipArchive::validate_header_consistency`]: crate::ZipArchive::validate_header_consistency
    #[must_use]
    pub fn enforce_header_consistency(mut self, enforce: bool) -> Self {
        self.files.enforce_header_consistency = enforce;
        self
    }
}

/// Iterator over the entries of a [`ZipIterable`].
///
/// Entries are yielded in central-directory order. Duplicate names are preserved. After an
/// error, the iterator stops yielding entries until [`ZipIterable::files`] is called again.
/// When [`ZipIterable::enforce_header_consistency`] is enabled, each entry is validated against
/// its local file header before it is yielded.
#[derive(Debug)]
pub struct ZipIterableFiles<R> {
    reader: R,
    central_directory: CentralDirectoryInfo,
    next_header_start: u64,
    remaining: usize,
    enforce_header_consistency: bool,
}

impl<R: Read + Seek> ZipIterableFiles<R> {
    fn new(reader: R, central_directory: CentralDirectoryInfo) -> Self {
        let next_header_start = central_directory.directory_start;
        let remaining = central_directory.number_of_files;
        Self {
            reader,
            central_directory,
            next_header_start,
            remaining,
            enforce_header_consistency: false,
        }
    }

    fn reset(&mut self) -> ZipResult<()> {
        self.next_header_start = self.central_directory.directory_start;
        self.remaining = self.central_directory.number_of_files;
        self.reader.seek(SeekFrom::Start(self.next_header_start))?;
        Ok(())
    }

    /// Number of entries that have not been yielded yet.
    #[must_use]
    pub fn len(&self) -> usize {
        self.remaining
    }

    /// Whether this pass has yielded every entry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.remaining == 0
    }

    /// Borrow the underlying reader.
    ///
    /// The iterator seeks back to the next central-directory header before parsing it, so using
    /// this reader to read entry contents does not disturb iteration. This is meant to be used
    /// with [`ZipFileEntry::with_reader`].
    pub fn reader_mut(&mut self) -> &mut R {
        &mut self.reader
    }

    fn parse_entry(
        &mut self,
        central_header_start: u64,
    ) -> ZipResult<(ZipFileEntry<'static>, u64)> {
        self.reader.seek(SeekFrom::Start(central_header_start))?;
        let block = ZipCentralEntryBlock::parse(&mut self.reader)?;
        let next = central_header_start
            .checked_add((size_of::<Magic>() + size_of::<ZipCentralEntryBlock>()) as u64)
            .and_then(|next| next.checked_add(u64::from(block.file_name_length)))
            .and_then(|next| next.checked_add(u64::from(block.extra_field_length)))
            .and_then(|next| next.checked_add(u64::from(block.file_comment_length)))
            .ok_or_else(|| invalid!("Central directory entry offset overflows u64"))?;

        let (data, file_name) = central_header_to_zip_file_inner(
            &mut self.reader,
            self.central_directory.archive_offset,
            central_header_start,
            block,
        )?;

        if self.enforce_header_consistency {
            data.validate_header_consistency(&mut self.reader, &file_name)?;
        }

        Ok((
            ZipFileEntry {
                file_name_raw: Cow::Owned(file_name),
                data: Cow::Owned(data),
            },
            next,
        ))
    }
}

impl<R: Read + Seek> Iterator for ZipIterableFiles<R> {
    type Item = ZipResult<ZipFileEntry<'static>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }

        match self.parse_entry(self.next_header_start) {
            Ok((entry, next)) => {
                self.next_header_start = next;
                self.remaining -= 1;
                Some(Ok(entry))
            }
            Err(e) => {
                // The reader position is unknown after a failed parse, so stop this pass.
                self.remaining = 0;
                Some(Err(e))
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
