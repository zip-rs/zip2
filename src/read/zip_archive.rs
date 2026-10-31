//! Code related to `ZipArchive`

use crate::compression::CompressionMethod;
use crate::format::MagicPositionCache;
use crate::format::blocks::{
    FixedSizeBlock, Zip32CentralDirectoryEnd, ZipCentralEntryBlock, ZipLocalEntryBlock,
};
use crate::format::find_central_directory_end;
use crate::format::magic::Magic;
use crate::read::config::Config;
use crate::read::extract::RootDirFilter;
use crate::read::magic_finder::{Forward, MagicFinder};
use crate::read::readers::{ZipFileReader, ZipFileSeekReader};
use crate::read::zipfile::{ZipFileEntry, ZipFileEntryWithDataDescriptor};
use crate::read::{
    ArchiveOffset, CentralDirectoryInfo, ZipFile, ZipFileSeek, ZipReadOptions,
    central_header_to_zip_file_inner,
};
use crate::result::{ZipError, ZipResult};
use crate::types::ZipFileData;
use crate::unstable::path_to_string;
use core::ops::Range;
use indexmap::IndexMap;
use std::borrow::Cow;
use std::io::{Read, Seek, SeekFrom};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Immutable metadata about a `ZipArchive`.
#[derive(Debug)]
pub struct ZipArchiveMetadata {
    pub(crate) files: IndexMap<Box<[u8]>, ZipFileData>,
    /// The raw file count (before inserting in the IndexMap) - used to detect duplicates
    pub(crate) raw_file_count: usize,
    pub(crate) offset: u64,
    pub(crate) dir_start: u64,
    // This isn't yet used anywhere, but it is here for use cases in the future.
    #[allow(dead_code)]
    pub(crate) config: Config,
    pub(crate) comment: Box<[u8]>,
    pub(crate) zip64_extensible_data_sector: Option<Box<[u8]>>,
}

impl ZipArchiveMetadata {
    /// Number of files contained in the central directory.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether the central directory contains no files.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get a central directory entry by index
    pub fn entry(&self, index: usize) -> ZipResult<ZipFileEntry<'_>> {
        let (file_name_raw, data) = self.files.get_index(index).ok_or(ZipError::FileNotFound)?;
        Ok(ZipFileEntry {
            file_name_raw: Cow::Borrowed(file_name_raw),
            data: Cow::Borrowed(data),
        })
    }

    /// Whether the central directory contains duplicate file names.
    #[must_use]
    pub fn has_duplicate_names(&self) -> bool {
        self.raw_file_count > self.files.len()
    }
}

#[derive(Debug)]
pub(crate) struct SharedBuilder {
    pub(crate) files: IndexMap<Box<[u8]>, ZipFileData>,
    pub(crate) raw_file_count: usize,
    pub(super) offset: u64,
    pub(super) dir_start: u64,
    // This isn't yet used anywhere, but it is here for use cases in the future.
    #[allow(dead_code)]
    pub(super) config: Config,
}

impl SharedBuilder {
    pub(crate) fn build(
        self,
        comment: Box<[u8]>,
        zip64_extensible_data_sector: Option<Box<[u8]>>,
    ) -> ZipArchiveMetadata {
        ZipArchiveMetadata {
            files: self.files,
            raw_file_count: self.raw_file_count,
            offset: self.offset,
            dir_start: self.dir_start,
            config: self.config,
            comment,
            zip64_extensible_data_sector,
        }
    }
}

/// ZIP archive reader
///
/// At the moment, this type is cheap to clone if this is the case for the
/// reader it uses. However, this is not guaranteed by this crate and it may
/// change in the future.
///
/// ```
/// use std::io::{Read, Seek};
/// fn list_zip_contents(reader: impl Read + Seek) -> zip::result::ZipResult<()> {
///     let mut zip = zip::ZipArchive::new(reader)?;
///
///     for i in 0..zip.len() {
///         let mut file = zip.by_index(i)?;
///         let name = file.name()?;
///         println!("Filename: {}", name);
///         std::io::copy(&mut file, &mut std::io::stdout())?;
///     }
///
///     Ok(())
/// }
/// ```
#[derive(Clone, Debug)]
pub struct ZipArchive<R> {
    pub(super) reader: R,
    pub(crate) shared: Arc<ZipArchiveMetadata>,
}

impl<R> ZipArchive<R> {
    pub(crate) fn from_finalized_writer(
        files: IndexMap<Box<[u8]>, ZipFileData>,
        comment: Box<[u8]>,
        zip64_extensible_data_sector: Option<Box<[u8]>>,
        reader: R,
        central_start: u64,
    ) -> Self {
        let initial_offset = match files.first() {
            Some((_, file)) => file.header_start,
            None => central_start,
        };
        let raw_file_count = files.len();
        let shared = Arc::new(ZipArchiveMetadata {
            files,
            raw_file_count,
            offset: initial_offset,
            dir_start: central_start,
            config: Config {
                archive_offset: ArchiveOffset::Known(initial_offset),
            },
            comment,
            zip64_extensible_data_sector,
        });
        Self { reader, shared }
    }

    /// Total size of the files in the archive, if it can be known. Doesn't include directories or
    /// metadata.
    pub fn decompressed_size(&self) -> Option<u128> {
        let mut total = 0u128;
        for file in self.shared.files.values() {
            if file.is_using_data_descriptor() {
                return None;
            }
            total = total.checked_add(u128::from(file.uncompressed_size))?;
        }
        Some(total)
    }
}

use crate::result::invalid;

/// The next `valid` headers decode, then the one after them fails with `error`.
#[derive(Debug, Clone)]
struct FailedRun {
    valid: usize,
    error: ZipError,
}

/// Headers walked by rejected EOCD candidates, keyed by position and shared by all of them.
#[derive(Debug, Default)]
struct CentralDirectoryChains {
    runs: std::collections::HashMap<u64, FailedRun>,
}

impl CentralDirectoryChains {
    /// Marks the headers in `chain` as followed by `tail` and returns its error.
    fn fail(&mut self, chain: &[u64], tail: FailedRun) -> ZipError {
        for (i, &central_header_start) in chain.iter().enumerate() {
            let run = FailedRun {
                valid: chain.len() - i + tail.valid,
                error: tail.error.clone(),
            };
            self.runs.insert(central_header_start, run);
        }
        tail.error
    }
}

impl<R: Read + Seek> ZipArchive<R> {
    /// Get the directory start offset and number of files. This is done in a
    /// separate function to ease the control flow design.
    pub(crate) fn get_metadata(config: Config, reader: &mut R) -> ZipResult<ZipArchiveMetadata> {
        // End of the probed region, initially set to the end of the file
        let file_len = reader.seek(SeekFrom::End(0))?;
        let mut last_err = None;
        let mut cd_chains = CentralDirectoryChains::default();

        const EOCD_SIG_BYTES: [u8; mem::size_of::<Magic>()] =
            Magic::CENTRAL_DIRECTORY_END_SIGNATURE.to_le_bytes();
        const EOCD64_SIG_BYTES: [u8; mem::size_of::<Magic>()] =
            Magic::ZIP64_CENTRAL_DIRECTORY_END_SIGNATURE.to_le_bytes();
        const CDFH_SIG_BYTES: [u8; mem::size_of::<Magic>()] =
            Magic::CENTRAL_DIRECTORY_HEADER_SIGNATURE.to_le_bytes();

        // Archive-comment limit is 64 KiB.
        let min_eocd_position = file_len.saturating_sub(
            size_of::<Zip32CentralDirectoryEnd>() as u64 + size_of::<Magic>() as u64 + (1 << 16),
        );

        let mut eocd_finder = MagicFinder::new(&EOCD_SIG_BYTES, min_eocd_position, file_len);
        let mut cdfh_cache = MagicPositionCache::new(CDFH_SIG_BYTES);
        let mut eocd64_cache = MagicPositionCache::new(EOCD64_SIG_BYTES);
        loop {
            // Find the EOCD and possibly EOCD64 entries and determine the archive offset.
            let cde = match find_central_directory_end(
                reader,
                config.archive_offset,
                file_len,
                &mut eocd_finder,
                &mut cdfh_cache,
                &mut eocd64_cache,
            ) {
                Ok(cde) => cde,
                Err(e) => {
                    // return the previous error first (if there is)
                    return Err(last_err.unwrap_or(e));
                }
            };

            // Turn EOCD into internal representation.
            match CentralDirectoryInfo::try_from(&cde)
                .and_then(|info| Self::read_central_header(&info, config, reader, &mut cd_chains))
            {
                Ok(shared) => {
                    let zip64_extensible_data_sector = if let Some(eocd64) = cde.eocd64 {
                        eocd64.data.zip64_extensible_data_sector
                    } else {
                        None
                    };
                    return Ok(
                        shared.build(cde.eocd.data.zip_file_comment, zip64_extensible_data_sector)
                    );
                }
                Err(e) => {
                    // Other readers use a record whose CD ends at it, so don't fall back past it.
                    if cde.central_directory_abuts_end() {
                        return Err(e);
                    }
                    last_err = Some(e);
                }
            }
            // The record is likely a stray signature (e.g. in a comment or trailing garbage),
            // try to find an earlier one
            continue;
        }
    }

    /// Decodes the header at `central_header_start`, where `reader` must be positioned. The
    /// returned entry's local header offset isn't shifted by the archive offset yet.
    fn read_one_central_header(
        reader: &mut R,
        central_header_start: u64,
    ) -> ZipResult<(Box<[u8]>, ZipFileData, u64)> {
        let block = ZipCentralEntryBlock::parse(reader)?;
        let next = central_header_start
            + (size_of::<Magic>() + size_of::<ZipCentralEntryBlock>()) as u64
            + u64::from(block.file_name_length)
            + u64::from(block.extra_field_length)
            + u64::from(block.file_comment_length);
        let (file, file_name_raw) =
            central_header_to_zip_file_inner(reader, 0, central_header_start, block)?;
        // Check against this header's position so the result is the same for every candidate.
        if file
            .header_start
            .checked_add(central_header_start)
            .is_none()
        {
            return Err(invalid!("Archive header is too large"));
        }
        Ok((file_name_raw.into_boxed_slice(), file, next))
    }

    fn read_central_header(
        dir_info: &CentralDirectoryInfo,
        config: Config,
        reader: &mut R,
        cd_chains: &mut CentralDirectoryChains,
    ) -> Result<SharedBuilder, ZipError> {
        // If the parsed number of files is greater than the offset then
        // something fishy is going on and we shouldn't trust number_of_files.
        // Furthermore, each CDFH requires at least 46 bytes (4 bytes magic + 42 bytes ZipCentralEntryBlock).
        let max_files = usize::try_from(dir_info.directory_size / 46).unwrap_or(usize::MAX);
        let dir_start_bound = usize::try_from(dir_info.directory_start).unwrap_or(usize::MAX);
        let file_capacity = if dir_info.number_of_files > dir_start_bound {
            0
        } else {
            dir_info.number_of_files.min(max_files)
        };

        if dir_info.disk_number != dir_info.disk_with_central_directory {
            return Err(ZipError::UnsupportedArchive(
                "Support for multi-disk files is not implemented",
            ));
        }

        if file_capacity.saturating_mul(size_of::<ZipFileData>()) > isize::MAX as usize {
            return Err(ZipError::UnsupportedArchive("Oversized central directory"));
        }

        // Decode independently of the candidate so later candidates can reuse `cd_chains`.
        let mut files = IndexMap::with_capacity(file_capacity);
        let mut chain = Vec::with_capacity(file_capacity);
        let mut central_header_start = dir_info.directory_start;
        reader.seek(SeekFrom::Start(central_header_start))?;
        while chain.len() < dir_info.number_of_files {
            // An earlier candidate dropped these headers; decode them again for this last one.
            if let Some(run) = cd_chains.runs.get(&central_header_start)
                && chain.len() + run.valid < dir_info.number_of_files
            {
                let run = run.clone();
                return Err(cd_chains.fail(&chain, run));
            }
            match Self::read_one_central_header(reader, central_header_start) {
                Ok((file_name_raw, file, next)) => {
                    chain.push(central_header_start);
                    files.insert(file_name_raw, file);
                    central_header_start = next;
                }
                Err(e) => {
                    let failed_here = FailedRun { valid: 0, error: e };
                    cd_chains
                        .runs
                        .insert(central_header_start, failed_here.clone());
                    return Err(cd_chains.fail(&chain, failed_here));
                }
            }
        }

        for file in files.values_mut() {
            file.header_start = file
                .header_start
                .checked_add(dir_info.archive_offset)
                .ok_or_else(|| invalid!("Archive header is too large"))?;
        }

        Ok(SharedBuilder {
            files,
            raw_file_count: chain.len(),
            offset: dir_info.archive_offset,
            dir_start: dir_info.directory_start,
            config,
        })
    }

    /// Returns the verification value and salt for the AES encryption of the file
    ///
    /// It fails if the file number is invalid.
    ///
    /// # Returns
    ///
    /// - None if the file is not encrypted with AES
    #[cfg(feature = "aes-crypto")]
    pub fn get_aes_verification_key_and_salt(
        &mut self,
        file_number: usize,
    ) -> ZipResult<Option<crate::aes::AesInfo>> {
        let (_, data) = self
            .shared
            .files
            .get_index(file_number)
            .ok_or(ZipError::FileNotFound)?;

        let limit_reader = data.find_content(&mut self.reader)?;
        match data.aes_settings() {
            None => Ok(None),
            Some((aes_mode, _)) => {
                let (verification_value, salt) =
                    crate::aes::AesReader::new(limit_reader, aes_mode, data.compressed_size)?
                        .get_verification_value_and_salt()?;
                let aes_info = crate::aes::AesInfo {
                    aes_mode,
                    verification_value,
                    salt,
                };
                Ok(Some(aes_info))
            }
        }
    }

    /// Read a ZIP archive, collecting the files it contains.
    ///
    /// This uses the central directory record of the ZIP file, and ignores local file headers.
    ///
    /// A default [`Config`] is used.
    pub fn new(reader: R) -> ZipResult<ZipArchive<R>> {
        Self::with_config(Config::default(), reader)
    }

    /// Get the metadata associated with the ZIP archive.
    ///
    /// This can be used with [`Self::unsafe_new_with_metadata`] to create a new reader over the
    /// same file without needing to reparse the metadata.
    pub fn metadata(&self) -> Arc<ZipArchiveMetadata> {
        self.shared.clone()
    }

    /// Read a ZIP archive using the given `metadata`.
    ///
    /// This is useful for creating multiple readers over the same file without
    /// needing to reparse the metadata.
    ///
    /// # Safety
    /// `unsafe` is used here to indicate that `reader` and `metadata` could
    /// potentially be incompatible, and it is left to the user to ensure they are.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use std::fs;
    /// use rayon::prelude::*;
    ///
    /// const FILE_NAME: &str = "my_data.zip";
    ///
    /// let file = fs::File::open(FILE_NAME).unwrap();
    /// let mut archive = zip::ZipArchive::new(file).unwrap();
    ///
    /// let file_names = (0..archive.len())
    ///     .into_par_iter()
    ///     .map_init({
    ///         let metadata = archive.metadata();
    ///         move || {
    ///             let file = fs::File::open(FILE_NAME).unwrap();
    ///             unsafe { zip::ZipArchive::unsafe_new_with_metadata(file, metadata.clone()) }
    ///         }},
    ///         |archive, i| {
    ///             let mut file = archive.by_index(i).unwrap();
    ///             file.enclosed_name()
    ///         }
    ///     )
    ///     .filter_map(|name| name)
    ///     .collect::<Vec<_>>();
    /// ```
    pub unsafe fn unsafe_new_with_metadata(reader: R, metadata: Arc<ZipArchiveMetadata>) -> Self {
        Self {
            reader,
            shared: metadata,
        }
    }

    /// Read a ZIP archive providing a read configuration, collecting the files it contains.
    ///
    /// This uses the central directory record of the ZIP file, and ignores local file headers.
    pub fn with_config(config: Config, mut reader: R) -> ZipResult<ZipArchive<R>> {
        let shared = Self::get_metadata(config, &mut reader)?;

        Ok(ZipArchive {
            reader,
            shared: Arc::new(shared),
        })
    }

    /// Number of files contained in this zip.
    pub fn len(&self) -> usize {
        self.shared.len()
    }

    /// Get the starting offset of the zip central directory.
    pub fn central_directory_start(&self) -> u64 {
        self.shared.dir_start
    }

    /// Whether this zip archive contains no files
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether this zip archive contains duplicate file names in its central directory.
    #[must_use]
    pub fn has_duplicate_names(&self) -> bool {
        self.shared.has_duplicate_names()
    }

    /// Get the offset from the beginning of the underlying reader that this zip begins at, in bytes.
    ///
    /// Normally this value is zero, but if the zip has arbitrary data prepended to it, then this value will be the size
    /// of that prepended data.
    pub fn offset(&self) -> u64 {
        self.shared.offset
    }

    /// Get the comment of the zip archive.
    pub fn comment(&self) -> &[u8] {
        &self.shared.comment
    }

    /// Get the ZIP64 extensible_data of the zip archive, if it is ZIP64.
    pub fn raw_zip64_extensible_data_sector(&self) -> Option<&[u8]> {
        self.shared.zip64_extensible_data_sector.as_deref()
    }

    /// Returns an iterator over all the file and directory names in this archive.
    pub fn file_names(&self) -> impl Iterator<Item = ZipResult<Cow<'_, str>>> {
        self.shared
            .files
            .iter()
            .map(|(file_name_raw, data)| data.name(file_name_raw))
    }

    /// Returns an iterator over all the raw file and directory names in this archive.
    pub fn file_names_raw(&self) -> impl Iterator<Item = &[u8]> {
        self.shared
            .files
            .iter()
            .map(|(file_name_raw, _data)| file_name_raw.as_ref())
    }

    /// Returns Ok(true) if any compressed data in this archive belongs to more than one file. This
    /// doesn't make the archive invalid, but some programs will refuse to decompress it because m
    /// copies of a block that decompresses to length n would occupy `m*n` bytes in the destination
    /// but require only an archive size of O(m + n) bytes.
    ///
    /// This will return true on an archive where `zip::write::shallow_copy_file` has been used,
    /// unless the file being copied has a compressed size of zero.
    ///
    /// Doesn't count cases where one file entry's header overlaps with another's header or data.
    /// To include those cases, use [ZipArchive::has_overlapping_entries] instead.
    pub fn has_overlapping_files(&mut self) -> ZipResult<bool> {
        self.has_overlap(false)
    }

    /// Returns Ok(true) if any byte in this archive belongs to more than one file entry, including
    /// the header. Similar to [ZipArchive::has_overlapping_files] except that it also checks for
    /// header-with-header and header-with-data overlaps. These can be used to increase the number
    /// of files in a ZIP archive so that the extracted files' metadata takes up a larger multiple
    /// of the archive's size (but still a constant multiple, since seeking readers require a
    /// central-directory entry for each file and streaming readers process one entry at a time).
    ///
    /// This will return true on an archive where `zip::write::shallow_copy_file` has been used,
    /// even when the file being copied has a size of zero.
    pub fn has_overlapping_entries(&mut self) -> ZipResult<bool> {
        self.has_overlap(true)
    }

    fn has_overlap(&mut self, include_header: bool) -> ZipResult<bool> {
        let ranges = self.entry_byte_ranges(include_header)?;
        Ok(Self::ranges_overlap(&ranges, self.shared.dir_start))
    }

    fn entry_byte_ranges(&mut self, include_header: bool) -> ZipResult<Vec<Range<u64>>> {
        let mut ranges = Vec::<Range<u64>>::with_capacity(self.shared.files.len());
        for file in self.shared.files.values() {
            if !include_header && file.compressed_size == 0 {
                continue;
            }
            let data_start = file.data_start(&mut self.reader)?;
            let start = if include_header {
                file.header_start
            } else {
                data_start
            };
            let end =
                data_start
                    .checked_add(file.compressed_size)
                    .ok_or(ZipError::InvalidArchive(Cow::Borrowed(
                        "File data end offset overflows u64",
                    )))?;
            ranges.push(start..end);
        }
        ranges.sort_unstable_by_key(|r| (r.start, r.end));
        Ok(ranges)
    }

    fn ranges_overlap(ranges: &[Range<u64>], dir_start: u64) -> bool {
        ranges.iter().any(|r| r.end > dir_start) || ranges.windows(2).any(|w| w[0].end > w[1].start)
    }

    fn unoccupied_ranges(ranges: &[Range<u64>], end: u64) -> Vec<Range<u64>> {
        let mut unoccupied = Vec::new();
        let mut cursor = 0;
        for range in ranges {
            let gap_end = range.start.min(end);
            if cursor < gap_end {
                unoccupied.push(cursor..gap_end);
            }
            cursor = cursor.max(range.end.min(end));
            if cursor == end {
                break;
            }
        }
        if cursor < end {
            unoccupied.push(cursor..end);
        }
        unoccupied
    }

    fn scan_for_unlisted_local_headers(&mut self, gaps: &[Range<u64>]) -> ZipResult<bool> {
        const LOCAL_FILE_HEADER_SIGNATURE: [u8; size_of::<Magic>()] =
            Magic::LOCAL_FILE_HEADER_SIGNATURE.to_le_bytes();

        for gap in gaps {
            let mut finder =
                MagicFinder::<Forward<'_>>::new(&LOCAL_FILE_HEADER_SIGNATURE, gap.start, gap.end);
            while let Some(offset) = finder.next(&mut self.reader)? {
                if Self::is_valid_local_header_at(&mut self.reader, offset, gap.end)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn is_valid_local_header_at<T: Read + Seek + ?Sized>(
        reader: &mut T,
        offset: u64,
        range_end: u64,
    ) -> ZipResult<bool> {
        let fixed_header_end =
            offset.checked_add((size_of::<Magic>() + size_of::<ZipLocalEntryBlock>()) as u64);
        if fixed_header_end.is_none_or(|end| end > range_end) {
            return Ok(false);
        }
        reader.seek(SeekFrom::Start(offset))?;
        let block = match ZipLocalEntryBlock::parse(reader) {
            Ok(block) => block,
            Err(ZipError::Io(error)) if error.kind() != std::io::ErrorKind::UnexpectedEof => {
                return Err(ZipError::Io(error));
            }
            Err(_) => return Ok(false),
        };
        let variable_fields_len =
            u64::from(block.file_name_length) + u64::from(block.extra_field_length);
        Ok(!fixed_header_end
            .and_then(|end| end.checked_add(variable_fields_len))
            .is_none_or(|end| end > range_end))
    }

    /// Search for a file entry by name, decrypt with given password
    ///
    /// # Warning
    ///
    /// The implementation of the cryptographic algorithms has not
    /// gone through a correctness review, and you should assume it is insecure:
    /// passwords used with this API may be compromised.
    ///
    /// This function sometimes accepts wrong password. This is because the ZIP spec only allows us
    /// to check for a 1/256 chance that the password is correct.
    /// There are many passwords out there that will also pass the validity checks
    /// we are able to perform. This is a weakness of the `ZipCrypto` algorithm,
    /// due to its fairly primitive approach to cryptography.
    pub fn by_name_decrypt(&mut self, name: &str, password: &[u8]) -> ZipResult<ZipFile<'_, R>> {
        self.by_name_with_optional_password(name, Some(password))
    }

    /// Search for a file entry by name
    /// First try to get the index by the name as a binary representation of the name. If not found,
    /// try to find the name with the decoded version of the name. This is a fallback for name
    /// encoded with CP437.
    pub fn by_name(&mut self, name: &str) -> ZipResult<ZipFile<'_, R>> {
        self.by_name_with_optional_password(name, None)
    }

    /// Search for a file entry by its raw name bytes, without decoding the name.
    /// The running time of this operation doesn't scale with the number of files in the archive,
    /// only with the length of the name being looked up (since it is hashed).
    pub fn by_name_raw(&mut self, name: &[u8]) -> ZipResult<ZipFile<'_, R>> {
        let index = self
            .index_for_name_raw(name)
            .ok_or(ZipError::FileNotFound)?;
        self.by_index_raw(index)
    }

    /// Get the index of a file entry by name, if it's present.
    /// First try to get the index by the name as a binary representation of the name. If not found,
    /// try to find the name with the decoded version of the name. This is a fallback for name
    /// encoded with CP437.
    /// If you don't need to use the fallback, consider using [`Self::index_for_name_raw`] which doesn't
    /// have the fallback.
    #[inline]
    pub fn index_for_name(&self, name: &str) -> Option<usize> {
        self.shared.files.get_index_of(name.as_bytes()).or_else(|| {
            self.shared
                .files
                .iter()
                .position(|(raw, data)| data.name(raw).is_ok_and(|decoded| decoded == name))
        })
    }

    /// Get the index of a file entry by the raw name, if it's present. The running time of this
    /// operation doesn't scale with the number of files in the archive, only with the length of the
    /// name being looked up (since it is hashed).
    #[inline]
    pub fn index_for_name_raw(&self, name: &[u8]) -> Option<usize> {
        self.shared.files.get_index_of(name)
    }

    /// Get data and the data descriptor of the file
    /// This method will work if the zip is using the data descriptor signature (see 4.3.9.3)
    /// To access the data descriptor, the ZipFileData needs to have a correct `compressed_size` and
    /// the Data Descriptor should follow the data
    pub fn by_index_with_data_descriptor(
        &mut self,
        index: usize,
    ) -> ZipResult<ZipFileEntryWithDataDescriptor<'_>> {
        let (file_name_raw, data) = self
            .shared
            .files
            .get_index(index)
            .ok_or(ZipError::FileNotFound)?;

        let data_descriptor = data.get_data_descriptor(&mut self.reader)?;
        Ok(ZipFileEntryWithDataDescriptor {
            file_name_raw: Cow::Borrowed(file_name_raw),
            data: Cow::Borrowed(data),
            data_descriptor,
        })
    }

    /// Search for a file entry by path, decrypt with given password
    ///
    /// # Warning
    ///
    /// The implementation of the cryptographic algorithms has not
    /// gone through a correctness review, and you should assume it is insecure:
    /// passwords used with this API may be compromised.
    ///
    /// This function sometimes accepts wrong password. This is because the ZIP spec only allows us
    /// to check for a 1/256 chance that the password is correct.
    /// There are many passwords out there that will also pass the validity checks
    /// we are able to perform. This is a weakness of the `ZipCrypto` algorithm,
    /// due to its fairly primitive approach to cryptography.
    pub fn by_path_decrypt<T: AsRef<Path>>(
        &mut self,
        path: T,
        password: &[u8],
    ) -> ZipResult<ZipFile<'_, R>> {
        self.index_for_path(path)
            .ok_or(ZipError::FileNotFound)
            .and_then(|index| {
                self.by_index_with_options(index, ZipReadOptions::new().password(Some(password)))
            })
    }

    /// Search for a file entry by path
    pub fn by_path<T: AsRef<Path>>(&mut self, path: T) -> ZipResult<ZipFile<'_, R>> {
        self.index_for_path(path)
            .ok_or(ZipError::FileNotFound)
            .and_then(|index| self.by_index_with_options(index, ZipReadOptions::new()))
    }

    /// Get the index of a file entry by path, if it's present.
    #[inline(always)]
    pub fn index_for_path<T: AsRef<Path>>(&self, path: T) -> Option<usize> {
        let path_as_string = &path_to_string(path).ok()?;
        self.index_for_name(path_as_string)
    }

    /// Get the name of a file entry, if it's present.
    #[inline(always)]
    pub fn name_for_index(&self, index: usize) -> Option<ZipResult<Cow<'_, str>>> {
        self.shared
            .files
            .get_index(index)
            .map(|(file_name_raw, zip_file_data)| zip_file_data.name(file_name_raw))
    }

    /// Search for a file entry by name and return a seekable object.
    pub fn by_name_seek(&mut self, name: &str) -> ZipResult<ZipFileSeek<'_, R>> {
        self.by_index_seek(self.index_for_name(name).ok_or(ZipError::FileNotFound)?)
    }

    /// Search for a file entry by index and return a seekable object.
    pub fn by_index_seek(&mut self, index: usize) -> ZipResult<ZipFileSeek<'_, R>> {
        let reader = &mut self.reader;
        self.shared
            .files
            .get_index(index)
            .ok_or(ZipError::FileNotFound)
            .and_then(move |(file_name_raw, data)| {
                let seek_reader = match data.compression_method {
                    CompressionMethod::Stored => {
                        ZipFileSeekReader::Raw(data.find_content_seek(reader)?)
                    }
                    _ => {
                        return Err(ZipError::UnsupportedArchive(
                            "Seekable compressed files are not yet supported",
                        ));
                    }
                };
                Ok(ZipFileSeek {
                    file_name_raw: Cow::Borrowed(file_name_raw),
                    reader: seek_reader,
                    data: Cow::Borrowed(data),
                })
            })
    }

    fn by_name_with_optional_password<'a>(
        &'a mut self,
        name: &str,
        password: Option<&[u8]>,
    ) -> ZipResult<ZipFile<'a, R>> {
        let Some(index) = self.index_for_name(name) else {
            return Err(ZipError::FileNotFound);
        };
        self.by_index_with_options(index, ZipReadOptions::new().password(password))
    }

    /// Get a contained file by index, decrypt with given password
    ///
    /// # Warning
    ///
    /// The implementation of the cryptographic algorithms has not
    /// gone through a correctness review, and you should assume it is insecure:
    /// passwords used with this API may be compromised.
    ///
    /// This function sometimes accepts wrong password. This is because the ZIP spec only allows us
    /// to check for a 1/256 chance that the password is correct.
    /// There are many passwords out there that will also pass the validity checks
    /// we are able to perform. This is a weakness of the `ZipCrypto` algorithm,
    /// due to its fairly primitive approach to cryptography.
    pub fn by_index_decrypt(
        &mut self,
        file_number: usize,
        password: &[u8],
    ) -> ZipResult<ZipFile<'_, R>> {
        self.by_index_with_options(file_number, ZipReadOptions::new().password(Some(password)))
    }

    /// Get a contained file by index
    pub fn by_index(&mut self, file_number: usize) -> ZipResult<ZipFile<'_, R>> {
        self.by_index_with_options(file_number, ZipReadOptions::new())
    }

    /// Get a contained file by index
    pub fn by_index_data(&self, file_number: usize) -> ZipResult<ZipFileEntry<'_>> {
        self.shared.entry(file_number)
    }

    /// Get a contained file by index without decompressing it
    pub fn by_index_raw(&mut self, file_number: usize) -> ZipResult<ZipFile<'_, R>> {
        let reader = &mut self.reader;
        let (file_name_raw, data) = self
            .shared
            .files
            .get_index(file_number)
            .ok_or(ZipError::FileNotFound)?;
        Ok(ZipFile {
            file_name_raw: Cow::Borrowed(file_name_raw),
            reader: ZipFileReader::Raw(data.find_content(reader)?),
            data: Cow::Borrowed(data),
        })
    }

    /// Search for a file entry by name with options.
    pub fn by_name_with_options<'a>(
        &'a mut self,
        name: &str,
        options: ZipReadOptions<'_>,
    ) -> ZipResult<ZipFile<'a, R>> {
        let Some(index) = self.index_for_name(name) else {
            return Err(ZipError::FileNotFound);
        };
        self.by_index_with_options(index, options)
    }

    /// Get a contained file by index with options.
    pub fn by_index_with_options(
        &mut self,
        file_number: usize,
        options: ZipReadOptions<'_>,
    ) -> ZipResult<ZipFile<'_, R>> {
        let (file_name_raw, data) = self
            .shared
            .files
            .get_index(file_number)
            .ok_or(ZipError::FileNotFound)?;
        if options.enforce_header_consistency {
            if self.shared.has_duplicate_names() {
                return Err(invalid!(
                    "header inconsistency detected: duplicate file names in central directory"
                ));
            }
            data.validate_header_consistency(&mut self.reader, file_name_raw)?;
        }
        let limit_reader = data.find_content(&mut self.reader)?;
        let reader = data.make_reader(limit_reader, options)?;
        Ok(ZipFile {
            file_name_raw: Cow::Borrowed(file_name_raw),
            data: Cow::Borrowed(data),
            reader,
        })
    }

    /// Validates that local file headers match their corresponding central directory headers and
    /// that no local file headers are missing from the central directory.
    pub fn validate_header_consistency(&mut self) -> ZipResult<()> {
        if self.shared.has_duplicate_names() {
            return Err(invalid!(
                "header inconsistency detected: duplicate file names in central directory"
            ));
        }
        for (file_name_raw, data) in &self.shared.files {
            data.validate_header_consistency(&mut self.reader, file_name_raw)?;
        }
        let ranges = self.entry_byte_ranges(true)?;
        if Self::ranges_overlap(&ranges, self.shared.dir_start) {
            return Err(invalid!(
                "header inconsistency detected: archive contains overlapping entries"
            ));
        }
        let gaps = Self::unoccupied_ranges(&ranges, self.shared.dir_start);
        if self.scan_for_unlisted_local_headers(&gaps)? {
            return Err(invalid!(
                "header inconsistency detected: local file header missing from central directory"
            ));
        }
        Ok(())
    }

    /// Find the "root directory" of an archive if it exists, filtering out
    /// irrelevant entries when searching.
    ///
    /// Our definition of a "root directory" is a single top-level directory
    /// that contains the rest of the archive's entries. This is useful for
    /// extracting archives that contain a single top-level directory that
    /// you want to "unwrap" and extract directly.
    ///
    /// For a sensible default filter, you can use [`crate::read::root_dir_common_filter`].
    /// For a custom filter, see [`RootDirFilter`].
    pub fn root_dir(&self, filter: impl RootDirFilter) -> ZipResult<Option<PathBuf>> {
        let mut root_dir: Option<PathBuf> = None;

        for i in 0..self.len() {
            let (file_name_raw, file) = self
                .shared
                .files
                .get_index(i)
                .ok_or(ZipError::FileNotFound)?;
            let file_name = match file.name(file_name_raw) {
                Ok(file_name) => file_name,
                Err(_) => continue,
            };
            let path = match file.enclosed_name(&file_name) {
                Some(path) => path,
                None => return Ok(None),
            };

            if !filter(&path) {
                continue;
            }

            macro_rules! replace_root_dir {
                ($path:ident) => {
                    match &mut root_dir {
                        Some(root_dir) => {
                            if *root_dir != $path {
                                // We've found multiple root directories,
                                // abort.
                                return Ok(None);
                            }
                            continue;
                        }

                        None => {
                            root_dir = Some($path.into());
                            continue;
                        }
                    }
                };
            }

            // If this entry is located at the root of the archive...
            if path.components().count() == 1 {
                if file.is_dir(file_name_raw.as_ref()) {
                    // If it's a directory, it could be the root directory.
                    replace_root_dir!(path);
                }
                // If it's anything else, this archive does not have a
                // root directory.
                return Ok(None);
            }

            // Find the root directory for this entry.
            let mut path = path.as_path();
            while let Some(parent) = path.parent().filter(|path| *path != Path::new("")) {
                path = parent;
            }

            replace_root_dir!(path);
        }

        Ok(root_dir)
    }

    /// Unwrap and return the inner reader object
    ///
    /// The position of the reader is undefined.
    pub fn into_inner(self) -> R {
        self.reader
    }
}

#[cfg(test)]
mod tests {
    use crate::format::magic::Magic;
    use crate::format::{MagicPositionCache, find_central_directory_end};
    use crate::read::ArchiveOffset;
    use crate::read::magic_finder::{Backwards, MagicFinder};
    use std::io::{Cursor, Write};

    #[test]
    fn unlisted_local_header_with_malformed_extra_field_is_detected() {
        let mut bytes = Magic::LOCAL_FILE_HEADER_SIGNATURE.to_le_bytes().to_vec();
        let mut fixed_header = [0; 26];
        fixed_header[22..24].copy_from_slice(&1u16.to_le_bytes());
        fixed_header[24..26].copy_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&fixed_header);
        bytes.extend_from_slice(b"x");
        bytes.extend_from_slice(&[1, 0, 5, 0]);

        assert!(
            super::ZipArchive::<Cursor<Vec<u8>>>::is_valid_local_header_at(
                &mut Cursor::new(bytes.clone()),
                0,
                bytes.len() as u64,
            )
            .unwrap()
        );
    }

    #[test]
    fn invalid_offset() {
        use super::ZipArchive;
        use std::io::Cursor;

        let reader = ZipArchive::new(Cursor::new(include_bytes!(
            "../../tests/data/invalid_offset.zip"
        )));
        assert!(reader.is_err());
    }

    #[test]
    fn invalid_offset2() {
        use super::ZipArchive;
        use std::io::Cursor;

        let reader = ZipArchive::new(Cursor::new(include_bytes!(
            "../../tests/data/invalid_offset2.zip"
        )));
        assert!(reader.is_err());
    }

    #[test]
    fn zip64_with_leading_junk() {
        use super::ZipArchive;
        use std::io::Cursor;

        let reader = ZipArchive::new(Cursor::new(include_bytes!(
            "../../tests/data/zip64_demo.zip"
        )))
        .unwrap();
        assert_eq!(reader.len(), 1);
    }

    #[test]
    fn zip_contents() {
        use super::ZipArchive;
        use std::io::Cursor;

        let mut reader =
            ZipArchive::new(Cursor::new(include_bytes!("../../tests/data/mimetype.zip"))).unwrap();
        assert_eq!(reader.comment(), b"");
        assert_eq!(reader.by_index(0).unwrap().central_header_start(), 77);
    }

    #[test]
    fn metadata_entry_matches_by_index() {
        use super::ZipArchive;
        use crate::result::ZipError;
        use std::io::Cursor;

        let mut reader = ZipArchive::new(Cursor::new(include_bytes!(
            "../../tests/data/files_and_dirs.zip"
        )))
        .unwrap();
        let metadata = reader.metadata();
        assert_eq!(metadata.len(), reader.len());
        assert!(!metadata.is_empty());

        for i in 0..metadata.len() {
            let entry = metadata.entry(i).unwrap();
            let file = reader.by_index(i).unwrap();
            assert_eq!(entry.name_raw(), file.name_raw());
            assert_eq!(entry.size(), file.size());
            assert_eq!(entry.compressed_size(), file.compressed_size());
            assert_eq!(entry.central_header_start(), file.central_header_start());
        }

        assert!(matches!(
            metadata.entry(metadata.len()),
            Err(ZipError::FileNotFound)
        ));
    }

    #[test]
    fn metadata_empty_archive() {
        use super::ZipArchive;
        use crate::result::ZipError;
        use crate::write::ZipWriter;
        use std::io::Cursor;

        let writer = ZipWriter::new(Cursor::new(Vec::new()));
        let reader = ZipArchive::new(writer.finish().unwrap()).unwrap();
        let metadata = reader.metadata();
        assert_eq!(metadata.len(), 0);
        assert!(metadata.is_empty());
        assert!(matches!(metadata.entry(0), Err(ZipError::FileNotFound)));
    }

    #[test]
    fn zip_clone() {
        use super::ZipArchive;
        use std::io::Cursor;
        use std::io::Read;

        let mut reader1 =
            ZipArchive::new(Cursor::new(include_bytes!("../../tests/data/mimetype.zip"))).unwrap();
        let mut reader2 = reader1.clone();

        let mut file1 = reader1.by_index(0).unwrap();
        let mut file2 = reader2.by_index(0).unwrap();

        let t = file1.last_modified().unwrap();
        assert_eq!(
            (
                t.year(),
                t.month(),
                t.day(),
                t.hour(),
                t.minute(),
                t.second()
            ),
            (1980, 1, 1, 0, 0, 0)
        );

        let mut buf1 = [0; 5];
        let mut buf2 = [0; 5];
        let mut buf3 = [0; 5];
        let mut buf4 = [0; 5];

        file1.read_exact(&mut buf1).unwrap();
        file2.read_exact(&mut buf2).unwrap();
        file1.read_exact(&mut buf3).unwrap();
        file2.read_exact(&mut buf4).unwrap();

        assert_eq!(buf1, buf2);
        assert_eq!(buf3, buf4);
        assert_ne!(buf1, buf3);
    }

    #[test]
    fn file_and_dir_predicates() {
        use super::ZipArchive;
        use std::io::Cursor;

        let mut zip = ZipArchive::new(Cursor::new(include_bytes!(
            "../../tests/data/files_and_dirs.zip"
        )))
        .unwrap();

        for i in 0..zip.len() {
            let zip_file = zip.by_index(i).unwrap();
            let full_name = zip_file.enclosed_name().unwrap();
            let file_name = full_name.file_name().unwrap().to_str().unwrap();
            assert!(
                (file_name.starts_with("dir") && zip_file.is_dir())
                    || (file_name.starts_with("file") && zip_file.is_file())
            );
        }
    }

    #[test]
    fn zip64_magic_in_filenames() {
        use super::ZipArchive;
        use std::io::Cursor;

        let files = vec![
            include_bytes!("../../tests/data/zip64_magic_in_filename_1.zip").to_vec(),
            include_bytes!("../../tests/data/zip64_magic_in_filename_2.zip").to_vec(),
            include_bytes!("../../tests/data/zip64_magic_in_filename_3.zip").to_vec(),
            include_bytes!("../../tests/data/zip64_magic_in_filename_4.zip").to_vec(),
            include_bytes!("../../tests/data/zip64_magic_in_filename_5.zip").to_vec(),
        ];
        // Although we don't allow adding files whose names contain the ZIP64 CDB-end or
        // CDB-end-locator signatures, we still read them when they aren't genuinely ambiguous.
        for file in files {
            ZipArchive::new(Cursor::new(file)).unwrap();
        }
    }

    /// test case to ensure we don't preemptively over allocate based on the
    /// declared number of files in the CDE of an invalid zip when the number of
    /// files declared is more than the alleged offset in the CDE
    #[test]
    fn invalid_cde_number_of_files_allocation_smaller_offset() {
        use super::ZipArchive;
        use std::io::Cursor;

        let reader = ZipArchive::new(Cursor::new(include_bytes!(
            "../../tests/data/invalid_cde_number_of_files_allocation_smaller_offset.zip"
        )));
        assert!(reader.is_err() || reader.unwrap().is_empty());
    }

    /// test case to ensure we don't preemptively over allocate based on the
    /// declared number of files in the CDE of an invalid zip when the number of
    /// files declared is less than the alleged offset in the CDE
    #[test]
    fn invalid_cde_number_of_files_allocation_greater_offset() {
        use super::ZipArchive;
        use std::io::Cursor;

        let reader = ZipArchive::new(Cursor::new(include_bytes!(
            "../../tests/data/invalid_cde_number_of_files_allocation_greater_offset.zip"
        )));
        assert!(reader.is_err());
    }

    #[cfg(feature = "deflate64")]
    #[test]
    fn deflate64_index_out_of_bounds() {
        use super::ZipArchive;
        use std::io::Cursor;

        let mut reader = ZipArchive::new(Cursor::new(include_bytes!(
            "../../tests/data/raw_deflate64_index_out_of_bounds.zip"
        )))
        .unwrap();
        let mut file = reader.by_index(0).unwrap();
        std::io::copy(&mut file, &mut std::io::sink()).expect_err("Invalid file");
    }

    #[cfg(feature = "deflate64")]
    #[test]
    fn deflate64_not_enough_space() {
        use super::ZipArchive;
        use std::io::Cursor;

        ZipArchive::new(Cursor::new(include_bytes!(
            "../../tests/data/deflate64_issue_25.zip"
        )))
        .expect_err("Invalid file");
    }

    #[cfg(feature = "deflate-flate2")]
    #[test]
    fn test_read_with_data_descriptor() {
        use super::ZipArchive;
        use std::io::Cursor;
        use std::io::Read;

        let mut reader = ZipArchive::new(Cursor::new(include_bytes!(
            "../../tests/data/data_descriptor.zip"
        )))
        .unwrap();
        let mut decompressed = [0u8; 16];
        let mut file = reader.by_index(0).unwrap();
        assert_eq!(file.read(&mut decompressed).unwrap(), 12);
    }

    /// Cannot use fs with miri CI
    #[cfg(not(miri))]
    #[test]
    fn test_central_directory_before_file_data() {
        use super::ZipArchive;
        use crate::CompressionMethod;
        use crate::read::{ArchiveOffset, Config};
        use crate::write::{SimpleFileOptions, ZipWriter};
        use std::io::Cursor;
        use std::io::{Read, Write};

        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "payload.txt",
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(b"optimized layout").unwrap();
        let normal = writer.finish().unwrap().into_inner();

        // Move the central directory before the local header and its data.
        // The EOCD remains at the end, as in Firefox's optimized ZIP layout.
        let end_start = normal.len() - 22;
        let central_start =
            u32::from_le_bytes(normal[end_start + 16..end_start + 20].try_into().unwrap()) as usize;
        let mut central = normal[central_start..end_start].to_vec();
        let local = &normal[..central_start];
        let mut end = normal[end_start..].to_vec();
        assert_eq!(&central[..4], b"PK\x01\x02");
        assert_eq!(&local[..4], b"PK\x03\x04");
        let central_len = u32::try_from(central.len()).unwrap();
        central[42..46].copy_from_slice(&central_len.to_le_bytes());
        end[16..20].copy_from_slice(&0u32.to_le_bytes());
        let optimized = [central.as_slice(), local, end.as_slice()].concat();
        assert_eq!(&optimized[..4], b"PK\x01\x02");
        assert_eq!(&optimized[central.len()..central.len() + 4], b"PK\x03\x04");

        let mut archive = ZipArchive::with_config(
            Config {
                archive_offset: ArchiveOffset::Known(0),
            },
            Cursor::new(optimized),
        )
        .unwrap();
        assert_eq!(archive.len(), 1);
        let mut payload = String::new();
        archive
            .by_name("payload.txt")
            .unwrap()
            .read_to_string(&mut payload)
            .unwrap();
        assert_eq!(payload, "optimized layout");
    }

    #[test]
    fn test_ignore_encryption_flag() {
        use super::ZipArchive;
        use crate::ZipReadOptions;
        use std::io::Cursor;
        use std::io::Read;

        let mut reader = ZipArchive::new(Cursor::new(include_bytes!(
            "../../tests/data/ignore_encryption_flag.zip"
        )))
        .unwrap();

        // Get the file entry by ignoring its encryption flag.
        let mut file = reader
            .by_index_with_options(0, ZipReadOptions::new().ignore_encryption_flag(true))
            .unwrap();
        let mut contents = String::new();
        assert_eq!(file.name().unwrap(), "plaintext.txt");

        // The file claims it is encrypted, but it is not.
        assert!(file.encrypted());
        file.read_to_string(&mut contents).unwrap(); // ensures valid UTF-8
        assert_eq!(contents, "This file is not encrypted.\n");
    }

    #[test]
    fn test_zip32_with_false_positive_eocd64_locator() {
        let mut data = Vec::new();

        // 1. Write some garbage
        data.extend_from_slice(b"Some junk at the beginning");
        let archive_start = data.len() as u64;

        // 2. Write a Central Directory File Header (CDFH)
        let cdfh_pos = data.len() as u64;
        data.extend_from_slice(&Magic::CENTRAL_DIRECTORY_HEADER_SIGNATURE.to_le_bytes());
        data.extend_from_slice(&[0u8; 42]); // Rest of CDFH (fixed size 46)

        // 3. Write something that LOOKS like an EOCD64 locator but is NOT valid
        // This will be at eocd_offset - 20
        // We'll place it just before the EOCD.
        data.extend_from_slice(&Magic::ZIP64_CENTRAL_DIRECTORY_END_LOCATOR_SIGNATURE.to_le_bytes());
        data.extend_from_slice(&[0u8; 16]); // Rest of locator

        // 4. Write the Zip32 EOCD
        data.extend_from_slice(&Magic::CENTRAL_DIRECTORY_END_SIGNATURE.to_le_bytes());
        data.write_all(&0u16.to_le_bytes()).unwrap(); // disk number
        data.write_all(&0u16.to_le_bytes()).unwrap(); // disk with CD
        data.write_all(&1u16.to_le_bytes()).unwrap(); // number of files on this disk
        data.write_all(&1u16.to_le_bytes()).unwrap(); // number of files
        data.write_all(&(46u32).to_le_bytes()).unwrap(); // CD size
        data.write_all(&((cdfh_pos - archive_start) as u32).to_le_bytes())
            .unwrap(); // CD offset relative to archive start
        data.write_all(&0u16.to_le_bytes()).unwrap(); // comment length

        let file_len = data.len() as u64;
        let mut reader = Cursor::new(data);

        const CDE_SIG_BYTES: [u8; size_of::<Magic>()] =
            Magic::CENTRAL_DIRECTORY_END_SIGNATURE.to_le_bytes();
        const CDFH_SIG_BYTES: [u8; size_of::<Magic>()] =
            Magic::CENTRAL_DIRECTORY_HEADER_SIGNATURE.to_le_bytes();

        let mut eocd_finder = MagicFinder::<Backwards<'static>>::new(&CDE_SIG_BYTES, 0, file_len);
        let mut cdfh_cache = MagicPositionCache::new(CDFH_SIG_BYTES);
        let mut eocd64_cache =
            MagicPositionCache::new(Magic::ZIP64_CENTRAL_DIRECTORY_END_SIGNATURE.to_le_bytes());

        let result = find_central_directory_end(
            &mut reader,
            ArchiveOffset::Detect,
            file_len,
            &mut eocd_finder,
            &mut cdfh_cache,
            &mut eocd64_cache,
        );

        // This should succeed because even though there's a fake locator, it should fall back to Zip32
        // But current implementation might fail if it finds the locator and then fails to find EOCD64.
        assert!(result.is_ok(), "Expected Ok, got {:?}", result.err());
        let info = result.unwrap();
        assert_eq!(info.archive_offset, archive_start);
        assert!(info.eocd64.is_none());
    }
}
