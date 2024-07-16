//! Types for reading ZIP archives

use crate::compression::{CompressionMethod, Decompressor};
use crate::cp437::FromCp437;
use crate::extra_fields::{ExtendedTimestamp, ExtraField};
use crate::read::zip_archive::{Shared, SharedBuilder};
use crate::result::{ZipError, ZipResult};
use crate::spec::{
    self, FixedSizeBlock, Pod, Zip32CentralDirectoryEnd, Zip64CDELocatorBlock,
    Zip64CentralDirectoryEnd, ZIP64_ENTRY_THR,
};
use crate::types::{
    AesMode, AesModeInfo, AesVendorVersion, DateTime, System, ZipCentralEntryBlock, ZipFileData,
    ZipLocalEntryBlock,
};
use indexmap::IndexMap;
use std::ffi::OsString;
use std::fs::create_dir_all;
use std::io::{self, prelude::*, SeekFrom};
use std::mem;
use std::mem::size_of;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

mod config;

pub use config::*;

/// Provides high level API for reading from a stream.
pub(crate) mod stream;

#[cfg(feature = "lzma")]
pub(crate) mod lzma;

#[cfg(feature = "xz")]
pub(crate) mod xz;

// Put the struct declaration in a private module to convince rustdoc to display ZipArchive nicely
pub(crate) mod zip_archive {
    use indexmap::IndexMap;
    use std::sync::Arc;

    /// Extract immutable data from `ZipArchive` to make it cheap to clone
    #[derive(Debug)]
    pub(crate) struct Shared {
        pub(crate) files: IndexMap<Box<str>, super::ZipFileData>,
        pub(super) offset: u64,
        pub(super) dir_start: u64,
        // This isn't yet used anywhere, but it is here for use cases in the future.
        #[allow(dead_code)]
        pub(super) config: super::Config,
    }

    #[derive(Debug)]
    pub(crate) struct SharedBuilder {
        pub(crate) files: Vec<super::ZipFileData>,
        pub(super) offset: u64,
        pub(super) dir_start: u64,
        // This isn't yet used anywhere, but it is here for use cases in the future.
        #[allow(dead_code)]
        pub(super) config: super::Config,
    }

    impl SharedBuilder {
        pub fn build(self) -> Shared {
            let mut index_map = IndexMap::with_capacity(self.files.len());
            self.files.into_iter().for_each(|file| {
                index_map.insert(file.file_name.clone(), file);
            });
            Shared {
                files: index_map,
                offset: self.offset,
                dir_start: self.dir_start,
                config: self.config,
            }
        }
    }

    /// ZIP archive reader
    ///
    /// At the moment, this type is cheap to clone if this is the case for the
    /// reader it uses. However, this is not guaranteed by this crate and it may
    /// change in the future.
    ///
    /// ```no_run
    /// use std::io::prelude::*;
    /// fn list_zip_contents(reader: impl Read + Seek) -> zip::result::ZipResult<()> {
    ///     use zip::HasZipMetadata;
    ///     let mut zip = zip::ZipArchive::new(reader)?;
    ///
    ///     for i in 0..zip.len() {
    ///         let mut file = zip.by_index(i)?;
    ///         println!("Filename: {}", file.name());
    ///         std::io::copy(&mut file, &mut std::io::stdout())?;
    ///     }
    ///
    ///     Ok(())
    /// }
    /// ```
    #[derive(Clone, Debug)]
    pub struct ZipArchive<R> {
        pub(super) reader: R,
        pub(super) shared: Arc<Shared>,
        pub(super) comment: Arc<[u8]>,
    }
}

#[cfg(feature = "aes-crypto")]
use crate::aes::PWD_VERIFY_LENGTH;
use crate::extra_fields::UnicodeExtraField;
use crate::result::ZipError::{InvalidArchive, InvalidPassword};
use crate::types::ffi::S_IFLNK;
use crate::unstable::{path_to_string, LittleEndianReadExt};

use crate::crc32::Crc32Reader;
use crate::unstable::read::{
    construct_decompressing_reader, find_entry_content_range, ArchiveEntry, CryptoEntryReader,
    CryptoVariant, ZipEntry,
};

pub use zip_archive::ZipArchive;

pub(crate) fn find_data_start(
    data: &ZipFileData,
    reader: &mut (impl Read + Seek),
) -> Result<u64, ZipError> {
    // Go to start of data.
    reader.seek(SeekFrom::Start(data.header_start))?;

    // Parse static-sized fields and check the magic value.
    let block = ZipLocalEntryBlock::parse(reader)?;

    // Calculate the end of the local header from the fields we just parsed.
    let variable_fields_len =
        // Each of these fields must be converted to u64 before adding, as the result may
        // easily overflow a u16.
        block.file_name_length as u64 + block.extra_field_length as u64;
    let data_start =
        data.header_start + size_of::<ZipLocalEntryBlock>() as u64 + variable_fields_len;
    // Set the value so we don't have to read it again.
    match data.data_start.set(data_start) {
        Ok(()) => (),
        // If the value was already set in the meantime, ensure it matches (this is probably
        // unnecessary).
        Err(_) => {
            debug_assert_eq!(*data.data_start.get().unwrap(), data_start);
        }
    }
    Ok(data_start)
}

#[derive(Debug)]
pub(crate) struct CentralDirectoryInfo {
    pub(crate) archive_offset: u64,
    pub(crate) directory_start: u64,
    pub(crate) cde_position: u64,
    pub(crate) number_of_files: usize,
    pub(crate) disk_number: u32,
    pub(crate) disk_with_central_directory: u32,
    pub(crate) is_zip64: bool,
}

impl<R> ZipArchive<R> {
    pub(crate) fn from_finalized_writer(
        files: IndexMap<Box<str>, ZipFileData>,
        comment: Box<[u8]>,
        reader: R,
        central_start: u64,
    ) -> ZipResult<Self> {
        let initial_offset = match files.first() {
            Some((_, file)) => file.header_start,
            None => central_start,
        };
        let shared = Arc::new(Shared {
            files,
            offset: initial_offset,
            dir_start: central_start,
            config: Config {
                archive_offset: ArchiveOffset::Known(initial_offset),
            },
        });
        Ok(Self {
            reader,
            shared,
            comment: comment.into(),
        })
    }

    /// Total size of the files in the archive, if it can be known. Doesn't include directories or
    /// metadata.
    pub fn decompressed_size(&self) -> Option<u128> {
        let mut total = 0u128;
        for file in self.shared.files.values() {
            if file.using_data_descriptor {
                return None;
            }
            total = total.checked_add(file.uncompressed_size as u128)?;
        }
        Some(total)
    }

    const fn zip64_cde_len() -> usize {
        mem::size_of::<spec::Zip64CentralDirectoryEnd>()
            + mem::size_of::<spec::Zip64CentralDirectoryEndLocator>()
    }

    const fn order_lower_upper_bounds(a: u64, b: u64) -> (u64, u64) {
        if a > b {
            (b, a)
        } else {
            (a, b)
        }
    }

    /// Number of files contained in this zip.
    pub fn len(&self) -> usize {
        self.shared.files.len()
    }

    /// Whether this zip archive contains no files
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
        &self.comment
    }

    /// Returns an iterator over all the file and directory names in this archive.
    pub fn file_names(&self) -> impl Iterator<Item = &str> {
        self.shared.files.keys().map(|s| s.as_ref())
    }

    /// Get the index of a file entry by name, if it's present.
    #[inline(always)]
    pub fn index_for_name(&self, name: &str) -> Option<usize> {
        self.shared.files.get_index_of(name)
    }

    /// Get the index of a file entry by path, if it's present.
    #[inline(always)]
    pub fn index_for_path<T: AsRef<Path>>(&self, path: T) -> Option<usize> {
        self.index_for_name(&path_to_string(path))
    }

    #[inline(always)]
    fn index_for_name_err(&self, name: impl AsRef<str>) -> Result<usize, ZipError> {
        self.index_for_name(name.as_ref())
            .map(Ok)
            .unwrap_or(Err(ZipError::FileNotFound))
    }

    /// Get the name of a file entry, if it's present.
    #[inline(always)]
    pub fn name_for_index(&self, index: usize) -> Option<&str> {
        self.shared
            .files
            .get_index(index)
            .map(|(name, _)| name.as_ref())
    }
}

impl<R: Read + Seek> ZipArchive<R> {
    pub(crate) fn merge_contents<W: Write + Seek>(
        &mut self,
        mut w: W,
    ) -> ZipResult<IndexMap<Box<str>, ZipFileData>> {
        if self.shared.files.is_empty() {
            return Ok(IndexMap::new());
        }
        let mut new_files = self.shared.files.clone();
        /* The first file header will probably start at the beginning of the file, but zip doesn't
         * enforce that, and executable zips like PEX files will have a shebang line so will
         * definitely be greater than 0.
         *
         * assert_eq!(0, new_files[0].header_start); // Avoid this.
         */

        let first_new_file_header_start = w.stream_position()?;

        /* Push back file header starts for all entries in the covered files. */
        new_files.values_mut().try_for_each(|f| {
            /* This is probably the only really important thing to change. */
            f.header_start = f
                .header_start
                .checked_add(first_new_file_header_start)
                .ok_or(InvalidArchive(
                    "new header start from merge would have been too large",
                ))?;
            /* This is only ever used internally to cache metadata lookups (it's not part of the
             * zip spec), and 0 is the sentinel value. */
            f.central_header_start = 0;
            /* This is an atomic variable so it can be updated from another thread in the
             * implementation (which is good!). */
            if let Some(old_data_start) = f.data_start.take() {
                let new_data_start = old_data_start
                    .checked_add(first_new_file_header_start)
                    .ok_or(InvalidArchive(
                        "new data start from merge would have been too large",
                    ))?;
                f.data_start.get_or_init(|| new_data_start);
            }
            Ok::<_, ZipError>(())
        })?;

        /* Rewind to the beginning of the file.
         *
         * NB: we *could* decide to start copying from new_files[0].header_start instead, which
         * would avoid copying over e.g. any pex shebangs or other file contents that start before
         * the first zip file entry. However, zip files actually shouldn't care about garbage data
         * in *between* real entries, since the central directory header records the correct start
         * location of each, and keeping track of that math is more complicated logic that will only
         * rarely be used, since most zips that get merged together are likely to be produced
         * specifically for that purpose (and therefore are unlikely to have a shebang or other
         * preface). Finally, this preserves any data that might actually be useful.
         */
        self.reader.rewind()?;
        /* Find the end of the file data. */
        let length_to_read = self.shared.dir_start;
        /* Produce a Read that reads bytes up until the start of the central directory header. */
        let mut limited_raw = self.reader.by_ref().take(length_to_read);
        /* Copy over file data from source archive directly. */
        io::copy(&mut limited_raw, &mut w)?;

        /* Return the files we've just written to the data stream. */
        Ok(new_files)
    }

    fn get_directory_info_zip32(
        config: &Config,
        reader: &mut R,
        footer: &Zip32CentralDirectoryEnd,
        cde_start_pos: u64,
    ) -> ZipResult<CentralDirectoryInfo> {
        let archive_offset = match config.archive_offset {
            ArchiveOffset::Known(n) => n,
            ArchiveOffset::FromCentralDirectory | ArchiveOffset::Detect => {
                // Some zip files have data prepended to them, resulting in the
                // offsets all being too small. Get the amount of error by comparing
                // the actual file position we found the CDE at with the offset
                // recorded in the CDE.
                let mut offset = cde_start_pos
                    .checked_sub(footer.central_directory_size as u64)
                    .and_then(|x| x.checked_sub(footer.central_directory_offset as u64))
                    .ok_or(InvalidArchive("Invalid central directory size or offset"))?;

                if config.archive_offset == ArchiveOffset::Detect {
                    // Check whether the archive offset makes sense by peeking at the directory start. If it
                    // doesn't, fall back to using no archive offset. This supports zips with the central
                    // directory entries somewhere other than directly preceding the end of central directory.
                    reader.seek(SeekFrom::Start(
                        offset + footer.central_directory_offset as u64,
                    ))?;
                    let mut buf = [0; 4];
                    reader.read_exact(&mut buf)?;
                    if spec::Magic::from_le_bytes(buf)
                        != spec::Magic::CENTRAL_DIRECTORY_HEADER_SIGNATURE
                    {
                        offset = 0;
                    }
                }

                offset
            }
        };

        let directory_start = footer.central_directory_offset as u64 + archive_offset;
        let number_of_files = footer.number_of_files_on_this_disk as usize;
        Ok(CentralDirectoryInfo {
            archive_offset,
            directory_start,
            number_of_files,
            disk_number: footer.disk_number as u32,
            disk_with_central_directory: footer.disk_with_central_directory as u32,
            cde_position: cde_start_pos,
            is_zip64: false,
        })
    }

    const fn order_lower_upper_bounds(a: u64, b: u64) -> (u64, u64) {
        if a > b {
            (b, a)
        } else {
            (a, b)
        }
    }

    fn get_directory_info_zip64(
        config: &Config,
        reader: &mut R,
        cde_start_pos: u64,
    ) -> ZipResult<Vec<ZipResult<CentralDirectoryInfo>>> {
        // See if there's a ZIP64 footer. The ZIP64 locator if present will
        // have its signature 20 bytes in front of the standard footer. The
        // standard footer, in turn, is 22+N bytes large, where N is the
        // comment length. Therefore:
        reader.seek(SeekFrom::Start(
            cde_start_pos
                .checked_sub(size_of::<Zip64CDELocatorBlock>() as u64)
                .ok_or(InvalidArchive(
                    "No room for ZIP64 locator before central directory end",
                ))?,
        ))?;
        let locator64 = spec::Zip64CentralDirectoryEndLocator::parse(reader)?;

        // We need to reassess `archive_offset`. We know where the ZIP64
        // central-directory-end structure *should* be, but unfortunately we
        // don't know how to precisely relate that location to our current
        // actual offset in the file, since there may be junk at its
        // beginning. Therefore we need to perform another search, as in
        // read::Zip32CentralDirectoryEnd::find_and_parse, except now we search
        // forward. There may be multiple results because of Zip64 central-directory signatures in
        // ZIP comment data.

        let search_upper_bound = cde_start_pos
            .checked_sub(
                (size_of::<Zip64CentralDirectoryEnd>()
                    + size_of::<spec::Zip64CentralDirectoryEndLocator>()) as u64,
            )
            .ok_or(InvalidArchive(
                "File cannot contain ZIP64 central directory end",
            ))?;

        let (lower, upper) = Self::order_lower_upper_bounds(
            locator64.end_of_central_directory_offset,
            search_upper_bound,
        );

        let search_results = Zip64CentralDirectoryEnd::find_and_parse(reader, lower, upper)?;
        let results: Vec<ZipResult<CentralDirectoryInfo>> =
            search_results.into_iter().map(|(footer64, archive_offset)| {
                let archive_offset = match config.archive_offset {
                    ArchiveOffset::Known(n) => n,
                    ArchiveOffset::FromCentralDirectory => archive_offset,
                    ArchiveOffset::Detect => {
                        archive_offset.checked_add(footer64.central_directory_offset)
                            .and_then(|start| {
                                // Check whether the archive offset makes sense by peeking at the directory start.
                                //
                                // If any errors occur or no header signature is found, fall back to no offset to see if that works.
                                reader.seek(SeekFrom::Start(start)).ok()?;
                                let mut buf = [0; 4];
                                reader.read_exact(&mut buf).ok()?;
                                if spec::Magic::from_le_bytes(buf) != spec::Magic::CENTRAL_DIRECTORY_HEADER_SIGNATURE {
                                    None
                                } else {
                                    Some(archive_offset)
                                }
                            })
                        .unwrap_or(0)
                    }
                };
                let directory_start = footer64
                    .central_directory_offset
                    .checked_add(archive_offset)
                    .ok_or(InvalidArchive(
                        "Invalid central directory size or offset",
                    ))?;
                if directory_start > search_upper_bound {
                    Err(InvalidArchive(
                        "Invalid central directory size or offset",
                    ))
                } else if footer64.number_of_files_on_this_disk > footer64.number_of_files {
                    Err(InvalidArchive(
                        "ZIP64 footer indicates more files on this disk than in the whole archive",
                    ))
                } else if footer64.version_needed_to_extract > footer64.version_made_by {
                    Err(InvalidArchive(
                        "ZIP64 footer indicates a new version is needed to extract this archive than the \
                         version that wrote it",
                    ))
                } else {
                    Ok(CentralDirectoryInfo {
                        archive_offset,
                        directory_start,
                        number_of_files: footer64.number_of_files as usize,
                        disk_number: footer64.disk_number,
                        disk_with_central_directory: footer64.disk_with_central_directory,
                        cde_position: cde_start_pos,
                        is_zip64: true,
                    })
                }
            }).collect();
        Ok(results)
    }

    /// Get the directory start offset and number of files. This is done in a
    /// separate function to ease the control flow design.
    pub(crate) fn get_metadata(
        config: Config,
        reader: &mut R,
    ) -> ZipResult<(Zip32CentralDirectoryEnd, Shared)> {
        let mut invalid_errors_32 = Vec::new();
        let mut unsupported_errors_32 = Vec::new();
        let mut invalid_errors_64 = Vec::new();
        let mut unsupported_errors_64 = Vec::new();
        let mut ok_results = Vec::new();
        let cde_locations = Zip32CentralDirectoryEnd::find_and_parse(reader)?;
        cde_locations
            .into_vec()
            .into_iter()
            .for_each(|(footer, cde_start_pos)| {
                let zip32_result =
                    Self::get_directory_info_zip32(&config, reader, &footer, cde_start_pos);
                Self::sort_result(
                    zip32_result,
                    &mut invalid_errors_32,
                    &mut unsupported_errors_32,
                    &mut ok_results,
                    &footer,
                );
                let mut inner_results = Vec::with_capacity(1);
                // Check if file has a zip64 footer
                let zip64_vec_result =
                    Self::get_directory_info_zip64(&config, reader, cde_start_pos);
                Self::sort_result(
                    zip64_vec_result,
                    &mut invalid_errors_64,
                    &mut unsupported_errors_64,
                    &mut inner_results,
                    &(),
                );
                inner_results.into_iter().for_each(|(_, results)| {
                    results.into_iter().for_each(|result| {
                        Self::sort_result(
                            result,
                            &mut invalid_errors_64,
                            &mut unsupported_errors_64,
                            &mut ok_results,
                            &footer,
                        );
                    });
                });
            });
        ok_results.sort_by_key(|(_, result)| {
            (
                u64::MAX - result.cde_position, // try the last one first
                !result.is_zip64,               // try ZIP64 first
            )
        });
        let mut best_result = None;
        for (footer, result) in ok_results {
            let mut inner_result = Vec::with_capacity(1);
            let is_zip64 = result.is_zip64;
            Self::sort_result(
                Self::read_central_header(result, config, reader),
                if is_zip64 {
                    &mut invalid_errors_64
                } else {
                    &mut invalid_errors_32
                },
                if is_zip64 {
                    &mut unsupported_errors_64
                } else {
                    &mut unsupported_errors_32
                },
                &mut inner_result,
                &(),
            );
            if let Some((_, shared)) = inner_result.into_iter().next() {
                if shared.files.len() == footer.number_of_files as usize
                    || (is_zip64 && footer.number_of_files == ZIP64_ENTRY_THR as u16)
                {
                    best_result = Some((footer, shared));
                    break;
                } else {
                    if is_zip64 {
                        &mut invalid_errors_64
                    } else {
                        &mut invalid_errors_32
                    }
                    .push(InvalidArchive("wrong number of files"))
                }
            }
        }
        let Some((footer, shared)) = best_result else {
            return Err(unsupported_errors_32
                .into_iter()
                .chain(unsupported_errors_64)
                .chain(invalid_errors_32)
                .chain(invalid_errors_64)
                .next()
                .unwrap());
        };
        reader.seek(SeekFrom::Start(shared.dir_start))?;
        Ok((Rc::try_unwrap(footer).unwrap(), shared.build()))
    }

    fn read_central_header(
        dir_info: CentralDirectoryInfo,
        config: Config,
        reader: &mut R,
    ) -> Result<SharedBuilder, ZipError> {
        // If the parsed number of files is greater than the offset then
        // something fishy is going on and we shouldn't trust number_of_files.
        let file_capacity = if dir_info.number_of_files > dir_info.directory_start as usize {
            0
        } else {
            dir_info.number_of_files
        };
        if dir_info.disk_number != dir_info.disk_with_central_directory {
            return unsupported_zip_error("Support for multi-disk files is not implemented");
        }
        let mut files = Vec::with_capacity(file_capacity);
        reader.seek(SeekFrom::Start(dir_info.directory_start))?;
        for _ in 0..dir_info.number_of_files {
            let file = central_header_to_zip_file(reader, dir_info.archive_offset)?;
            files.push(file);
        }
        Ok(SharedBuilder {
            files,
            offset: dir_info.archive_offset,
            dir_start: dir_info.directory_start,
            config,
        })
    }

    fn sort_result<T, U: Clone>(
        result: ZipResult<T>,
        invalid_errors: &mut Vec<ZipError>,
        unsupported_errors: &mut Vec<ZipError>,
        ok_results: &mut Vec<(U, T)>,
        footer: &U,
    ) {
        match result {
            Err(ZipError::UnsupportedArchive(e)) => {
                unsupported_errors.push(ZipError::UnsupportedArchive(e))
            }
            Err(e) => invalid_errors.push(e),
            Ok(o) => ok_results.push((footer.clone(), o)),
        }
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
    ) -> ZipResult<Option<AesInfo>> {
        let entry = self.by_index_raw(file_number)?;
        entry.get_aes_verification_key_and_salt()
    }

    /// Read a ZIP archive, collecting the files it contains.
    ///
    /// This uses the central directory record of the ZIP file, and ignores local file headers.
    ///
    /// A default [`Config`] is used.
    pub fn new(reader: R) -> ZipResult<ZipArchive<R>> {
        Self::with_config(Default::default(), reader)
    }

    /// Read a ZIP archive providing a read configuration, collecting the files it contains.
    ///
    /// This uses the central directory record of the ZIP file, and ignores local file headers.
    pub fn with_config(config: Config, mut reader: R) -> ZipResult<ZipArchive<R>> {
        reader.seek(SeekFrom::Start(0))?;
        if let Ok((footer, shared)) = Self::get_metadata(config, &mut reader) {
            return Ok(ZipArchive {
                reader,
                shared: shared.into(),
                comment: footer.zip_file_comment.into(),
            });
        }
        Err(InvalidArchive("No valid central directory found"))
    }

    /// Extract a Zip archive into a directory, overwriting files if they
    /// already exist. Paths are sanitized with [`ZipFile::enclosed_name`].
    ///
    /// Extraction is not atomic. If an error is encountered, some of the files
    /// may be left on disk. However, on Unix targets, no newly-created directories with part but
    /// not all of their contents extracted will be readable, writable or usable as process working
    /// directories by any non-root user except you.
    ///
    /// On Unix and Windows, symbolic links are extracted correctly. On other platforms such as
    /// WebAssembly, symbolic links aren't supported, so they're extracted as normal files
    /// containing the target path in UTF-8.
    pub fn extract<P: AsRef<Path>>(&mut self, directory: P) -> ZipResult<()> {
        use std::fs;
        #[cfg(unix)]
        let mut files_by_unix_mode = Vec::new();
        for i in 0..self.len() {
            let mut file = self.by_index(i)?;
            let filepath = file
                .enclosed_name()
                .ok_or(InvalidArchive("Invalid file path"))?;

            let outpath = directory.as_ref().join(filepath);

            if file.is_dir() {
                Self::make_writable_dir_all(&outpath)?;
                continue;
            }
            let symlink_target = if file.is_symlink() && (cfg!(unix) || cfg!(windows)) {
                let mut target = Vec::with_capacity(file.size() as usize);
                /* FIXME: this is broken: needs to be .read_to_end(), otherwise it writes into an
                 * empty slice. */
                file.read_to_end(&mut target)?;
                Some(target)
            } else {
                None
            };
            drop(file);
            if let Some(p) = outpath.parent() {
                Self::make_writable_dir_all(p)?;
            }
            if let Some(target) = symlink_target {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;
                    let target = OsString::from_vec(target);
                    std::os::unix::fs::symlink(&target, outpath.as_path())?;
                }
                #[cfg(windows)]
                {
                    let Ok(target) = String::from_utf8(target) else {
                        return Err(ZipError::InvalidArchive("Invalid UTF-8 as symlink target"));
                    };
                    let target = target.into_boxed_str();
                    let target_is_dir_from_archive =
                        self.shared.files.contains_key(&target) && is_dir(&target);
                    let target_path = directory.as_ref().join(OsString::from(target.to_string()));
                    let target_is_dir = if target_is_dir_from_archive {
                        true
                    } else if let Ok(meta) = std::fs::metadata(&target_path) {
                        meta.is_dir()
                    } else {
                        false
                    };
                    if target_is_dir {
                        std::os::windows::fs::symlink_dir(target_path, outpath.as_path())?;
                    } else {
                        std::os::windows::fs::symlink_file(target_path, outpath.as_path())?;
                    }
                }
                continue;
            }
            let mut file = self.by_index(i)?;
            let mut outfile = fs::File::create(&outpath)?;
            io::copy(&mut file, &mut outfile)?;
            #[cfg(unix)]
            {
                // Check for real permissions, which we'll set in a second pass
                if let Some(mode) = file.unix_mode() {
                    files_by_unix_mode.push((outpath.clone(), mode));
                }
            }
        }
        #[cfg(unix)]
        {
            use std::cmp::Reverse;
            use std::os::unix::fs::PermissionsExt;

            if files_by_unix_mode.len() > 1 {
                // Ensure we update children's permissions before making a parent unwritable
                files_by_unix_mode.sort_by_key(|(path, _)| Reverse(path.clone()));
            }
            for (path, mode) in files_by_unix_mode.into_iter() {
                fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
            }
        }
        Ok(())
    }

    fn make_writable_dir_all<T: AsRef<Path>>(outpath: T) -> Result<(), ZipError> {
        create_dir_all(outpath.as_ref())?;
        #[cfg(unix)]
        {
            // Dirs must be writable until all normal files are extracted
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                outpath.as_ref(),
                std::fs::Permissions::from_mode(
                    0o700 | std::fs::metadata(outpath.as_ref())?.permissions().mode(),
                ),
            )?;
        }
        Ok(())
    }

    /// Search for a file entry by name
    pub fn by_name(&mut self, name: &str) -> ZipResult<ZipFile> {
        self.by_name_with_optional_password(name, None)
    }

    fn by_name_with_optional_password<'a>(
        &'a mut self,
        name: &str,
        password: Option<&[u8]>,
    ) -> ZipResult<ZipFile<'a>> {
        let index = self.index_for_name_err(name)?;
        self.by_index_with_optional_password(index, password)
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
    /// we are able to perform. This is a weakness of the ZipCrypto algorithm,
    /// due to its fairly primitive approach to cryptography.
    pub fn by_index_decrypt(
        &mut self,
        file_number: usize,
        password: &[u8],
    ) -> ZipResult<ZipFile<'_>> {
        self.by_index_with_optional_password(file_number, Some(password))
    }

    /// Get a contained file by index
    pub fn by_index(&mut self, file_number: usize) -> ZipResult<ZipFile<'_>> {
        self.by_index_with_optional_password(file_number, None)
    }

    /// Get a contained file by index without decompressing it
    pub fn by_index_raw(&mut self, file_number: usize) -> ZipResult<ZipFile<'_>> {
        let reader = &mut self.reader;
        let (_, data) = self
            .shared
            .files
            .get_index(file_number)
            .ok_or(ZipError::FileNotFound)?;
        Ok(ZipFile {
            crypto_reader: None,
            reader: ZipFileReader::Raw(find_content(data, reader)?),
            data: Cow::Borrowed(data),
        })
    }

    fn by_index_with_optional_password(
        &mut self,
        file_number: usize,
        mut password: Option<&[u8]>,
    ) -> ZipResult<ZipFile<'_>> {
        let (_, data) = self
            .shared
            .files
            .get_index(file_number)
            .ok_or(ZipError::FileNotFound)?;

        match (password, data.encrypted) {
            (None, true) => return Err(ZipError::UnsupportedArchive(ZipError::PASSWORD_REQUIRED)),
            (Some(_), false) => password = None, //Password supplied, but none needed! Discard.
            _ => {}
        }
        let limit_reader = find_content(data, &mut self.reader)?;

        let crypto_reader = make_crypto_reader(
            data.compression_method,
            data.crc32,
            data.last_modified_time,
            data.using_data_descriptor,
            limit_reader,
            password,
            data.aes_mode,
            #[cfg(feature = "aes-crypto")]
            data.compressed_size,
        )?;
        Ok(ZipFile {
            crypto_reader: Some(crypto_reader),
            reader: ZipFileReader::NoReader,
            data: Cow::Borrowed(data),
        })
    }

    /// Unwrap and return the inner reader object
    ///
    /// The position of the reader is undefined.
    pub fn into_inner(self) -> R {
        self.reader
    }
}

impl<R> ZipArchive<R>
where
    R: Read + Seek,
{
    /// Search for a file entry by name
    pub fn by_name(&mut self, name: impl AsRef<str>) -> ZipResult<ZipEntry<'_, impl Read + '_>> {
        let index = self.index_for_name_err(name)?;
        self.by_index(index)
    }

    /// Get a contained file by index
    pub fn by_index(&mut self, file_number: usize) -> ZipResult<ZipEntry<'_, impl Read + '_>> {
        let Self {
            ref mut reader,
            ref shared,
            ..
        } = self;
        let (_, data) = shared
            .files
            .get_index(file_number)
            .ok_or(ZipError::FileNotFound)?;

        /* Don't allow users to read out an encrypted entry without providing a password. */
        if data.encrypted {
            return Err(ZipError::UnsupportedArchive(ZipError::PASSWORD_REQUIRED));
        }

        let content = find_entry_content_range(data, reader)?;
        let entry_reader = construct_decompressing_reader(&data.compression_method, content)?;
        let crc32_reader = Crc32Reader::new(entry_reader, data.crc32);
        Ok(ZipEntry {
            data,
            reader: crc32_reader,
        })
    }

    /// Get a contained file by name without decompressing it
    pub fn by_name_raw(
        &mut self,
        name: impl AsRef<str>,
    ) -> ZipResult<ZipEntry<'_, impl Read + '_>> {
        let index = self.index_for_name_err(name)?;
        self.by_index_raw(index)
    }

    /// Get a contained file by index without decompressing it
    pub fn by_index_raw(&mut self, file_number: usize) -> ZipResult<ZipEntry<'_, impl Read + '_>> {
        let Self {
            ref mut reader,
            ref shared,
            ..
        } = self;
        let (_, data) = shared
            .files
            .get_index(file_number)
            .ok_or(ZipError::FileNotFound)?;
        let content = find_entry_content_range(data, reader)?;
        Ok(ZipEntry {
            data,
            reader: content,
        })
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
    /// we are able to perform. This is a weakness of the ZipCrypto algorithm,
    /// due to its fairly primitive approach to cryptography.
    pub fn by_name_decrypt(
        &mut self,
        name: impl AsRef<str>,
        password: &[u8],
    ) -> ZipResult<ZipEntry<'_, impl Read + '_>> {
        let index = self.index_for_name_err(name)?;
        self.by_index_decrypt(index, password)
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
    /// we are able to perform. This is a weakness of the ZipCrypto algorithm,
    /// due to its fairly primitive approach to cryptography.
    pub fn by_index_decrypt(
        &mut self,
        file_number: usize,
        password: &[u8],
    ) -> ZipResult<ZipEntry<'_, impl Read + '_>> {
        let Self {
            ref mut reader,
            ref shared,
            ..
        } = self;
        let (_, data) = shared
            .files
            .get_index(file_number)
            .ok_or(ZipError::FileNotFound)?;

        let content = find_entry_content_range(data, reader)?;

        let final_reader = if data.encrypted {
            let crypto_variant = CryptoVariant::from_data(data)?;
            let is_ae2_encrypted = crypto_variant.is_ae2_encrypted();
            let crypto_reader = crypto_variant.make_crypto_reader(password, content)?;
            let entry_reader =
                construct_decompressing_reader(&data.compression_method, crypto_reader)?;
            if is_ae2_encrypted {
                /* Ae2 voids crc checking: https://www.winzip.com/en/support/aes-encryption/ */
                CryptoEntryReader::Ae2Encrypted(entry_reader)
            } else {
                CryptoEntryReader::NonAe2Encrypted(Crc32Reader::new(entry_reader, data.crc32))
            }
        } else {
            /* Not encrypted, so do the same as in .by_index(): */
            let entry_reader = construct_decompressing_reader(&data.compression_method, content)?;
            CryptoEntryReader::Unencrypted(Crc32Reader::new(entry_reader, data.crc32))
        };

        Ok(ZipEntry {
            data,
            reader: final_reader,
        })
    }
}

/// Holds the AES information of a file in the zip archive
#[derive(Debug)]
#[cfg(feature = "aes-crypto")]
pub struct AesInfo {
    /// The AES encryption mode
    pub aes_mode: AesMode,
    /// The verification key
    pub verification_value: [u8; PWD_VERIFY_LENGTH],
    /// The salt
    pub salt: Vec<u8>,
}

const fn unsupported_zip_error<T>(detail: &'static str) -> ZipResult<T> {
    Err(ZipError::UnsupportedArchive(detail))
}

/// Parse a central directory entry to collect the information for the file.
pub(crate) fn central_header_to_zip_file<R: Read + Seek>(
    reader: &mut R,
    archive_offset: u64,
) -> ZipResult<ZipFileData> {
    let central_header_start = reader.stream_position()?;

    // Parse central header
    let block = ZipCentralEntryBlock::parse(reader)?;
    let file =
        central_header_to_zip_file_inner(reader, archive_offset, central_header_start, block)?;
    let central_header_end = reader.stream_position()?;
    let data_start = find_data_start(&file, reader)?;
    if data_start > central_header_start {
        return Err(InvalidArchive(
            "A file can't start after its central-directory header",
        ));
    }
    reader.seek(SeekFrom::Start(central_header_end))?;
    Ok(file)
}

#[inline]
fn read_variable_length_byte_field<R: Read>(reader: &mut R, len: usize) -> io::Result<Box<[u8]>> {
    let mut data = vec![0; len].into_boxed_slice();
    reader.read_exact(&mut data)?;
    Ok(data)
}

/// Parse a central directory entry to collect the information for the file.
pub(crate) fn central_header_to_zip_file_inner<R: Read>(
    reader: &mut R,
    archive_offset: u64,
    central_header_start: u64,
    block: ZipCentralEntryBlock,
) -> ZipResult<ZipFileData> {
    let ZipCentralEntryBlock {
        // magic,
        version_made_by,
        // version_to_extract,
        flags,
        compression_method,
        last_mod_time,
        last_mod_date,
        crc32,
        compressed_size,
        uncompressed_size,
        file_name_length,
        extra_field_length,
        file_comment_length,
        // disk_number,
        // internal_file_attributes,
        external_file_attributes,
        offset,
        ..
    } = block;

    let encrypted = flags & 1 == 1;
    let is_utf8 = flags & (1 << 11) != 0;
    let using_data_descriptor = flags & (1 << 3) != 0;

    let file_name_raw = read_variable_length_byte_field(reader, file_name_length as usize)?;
    let extra_field = read_variable_length_byte_field(reader, extra_field_length as usize)?;
    let file_comment_raw = read_variable_length_byte_field(reader, file_comment_length as usize)?;
    let file_name: Box<str> = match is_utf8 {
        true => String::from_utf8_lossy(&file_name_raw).into(),
        false => file_name_raw.clone().from_cp437(),
    };
    let file_comment: Box<str> = match is_utf8 {
        true => String::from_utf8_lossy(&file_comment_raw).into(),
        false => file_comment_raw.from_cp437(),
    };

    // Construct the result
    let mut result = ZipFileData {
        system: System::from((version_made_by >> 8) as u8),
        /* NB: this strips the top 8 bits! */
        version_made_by: version_made_by as u8,
        encrypted,
        using_data_descriptor,
        is_utf8,
        compression_method: CompressionMethod::parse_from_u16(compression_method),
        compression_level: None,
        last_modified_time: DateTime::try_from_msdos(last_mod_date, last_mod_time).ok(),
        crc32,
        compressed_size: compressed_size.into(),
        uncompressed_size: uncompressed_size.into(),
        file_name,
        file_name_raw,
        extra_field: Some(Arc::new(extra_field.to_vec())),
        central_extra_field: None,
        file_comment,
        header_start: offset.into(),
        extra_data_start: None,
        central_header_start,
        data_start: OnceLock::new(),
        external_attributes: external_file_attributes,
        large_file: false,
        aes_mode: None,
        aes_extra_data_start: 0,
        extra_fields: Vec::new(),
    };
    let stripped_extra_field = parse_extra_field(&mut result)?;
    result.extra_field = stripped_extra_field;

    let aes_enabled = result.compression_method == CompressionMethod::AES;
    if aes_enabled && result.aes_mode.is_none() {
        return Err(InvalidArchive(
            "AES encryption without AES extra data field",
        ));
    }

    // Account for shifted zip offsets.
    result.header_start = result
        .header_start
        .checked_add(archive_offset)
        .ok_or(InvalidArchive("Archive header is too large"))?;

    Ok(result)
}

pub(crate) fn parse_extra_field(file: &mut ZipFileData) -> ZipResult<Option<Arc<Vec<u8>>>> {
    let Some(ref extra_field) = file.extra_field else {
        return Ok(None);
    };
    let extra_field = extra_field.clone();
    let mut processed_extra_field = extra_field.clone();
    let len = extra_field.len();
    let mut reader = io::Cursor::new(&**extra_field);

    /* TODO: codify this structure into Zip64ExtraFieldBlock fields! */
    let mut position = reader.position() as usize;
    while position < len {
        let old_position = position;
        let remove = match parse_single_extra_field(file, &mut reader, position as u64, false) {
            Ok(b) => b,
            /* If we get an error reading too far, then assume this is an extra field we don't know
             * how to handle, and just return the remaining amount. */
            Err(ZipError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Ok(Some(processed_extra_field))
            }
            Err(e) => return Err(e),
        };
        position = reader.position() as usize;
        if remove {
            let remaining = len - (position - old_position);
            if remaining == 0 {
                return Ok(None);
            }
            let mut new_extra_field = Vec::with_capacity(remaining);
            new_extra_field.extend_from_slice(&extra_field[0..old_position]);
            new_extra_field.extend_from_slice(&extra_field[position..]);
            processed_extra_field = Arc::new(new_extra_field);
        }
    }
    Ok(Some(processed_extra_field))
}

pub(crate) fn parse_single_extra_field<R: Read>(
    file: &mut ZipFileData,
    reader: &mut R,
    bytes_already_read: u64,
    disallow_zip64: bool,
) -> ZipResult<bool> {
    let kind = reader.read_u16_le()?;
    let len = reader.read_u16_le()?;
    match kind {
        // Zip64 extended information extra field
        0x0001 => {
            if disallow_zip64 {
                return Err(InvalidArchive(
                    "Can't write a custom field using the ZIP64 ID",
                ));
            }
            file.large_file = true;
            let mut consumed_len = 0;
            if len >= 24 || file.uncompressed_size == spec::ZIP64_BYTES_THR {
                file.uncompressed_size = reader.read_u64_le()?;
                consumed_len += size_of::<u64>();
            }
            if len >= 24 || file.compressed_size == spec::ZIP64_BYTES_THR {
                file.compressed_size = reader.read_u64_le()?;
                consumed_len += size_of::<u64>();
            }
            if len >= 24 || file.header_start == spec::ZIP64_BYTES_THR {
                file.header_start = reader.read_u64_le()?;
                consumed_len += size_of::<u64>();
            }
            let Some(leftover_len) = (len as usize).checked_sub(consumed_len) else {
                return Err(InvalidArchive("ZIP64 extra-data field is the wrong length"));
            };
            reader.read_exact(&mut vec![0u8; leftover_len])?;
            return Ok(true);
        }
        0x9901 => {
            // AES
            if len != 7 {
                return Err(ZipError::UnsupportedArchive(
                    "AES extra data field has an unsupported length",
                ));
            }
            let vendor_version = reader.read_u16_le()?;
            let vendor_id = reader.read_u16_le()?;
            let mut out = [0u8];
            reader.read_exact(&mut out)?;
            let aes_mode = out[0];
            let compression_method = CompressionMethod::parse_from_u16(reader.read_u16_le()?);

            if vendor_id != 0x4541 {
                return Err(InvalidArchive("Invalid AES vendor"));
            }
            let vendor_version = match vendor_version {
                0x0001 => AesVendorVersion::Ae1,
                0x0002 => AesVendorVersion::Ae2,
                _ => return Err(InvalidArchive("Invalid AES vendor version")),
            };
            match aes_mode {
                0x01 => {
                    file.aes_mode = Some(AesModeInfo {
                        aes_mode: AesMode::Aes128,
                        vendor_version,
                        compression_method,
                    })
                }
                0x02 => {
                    file.aes_mode = Some(AesModeInfo {
                        aes_mode: AesMode::Aes192,
                        vendor_version,
                        compression_method,
                    })
                }
                0x03 => {
                    file.aes_mode = Some(AesModeInfo {
                        aes_mode: AesMode::Aes256,
                        vendor_version,
                        compression_method,
                    })
                }
                _ => return Err(InvalidArchive("Invalid AES encryption strength")),
            };
            file.compression_method = compression_method;
            file.aes_extra_data_start = bytes_already_read;
        }
        0x5455 => {
            // extended timestamp
            // https://libzip.org/specifications/extrafld.txt

            file.extra_fields.push(ExtraField::ExtendedTimestamp(
                ExtendedTimestamp::try_from_reader(reader, len)?,
            ));
        }
        0x6375 => {
            // Info-ZIP Unicode Comment Extra Field
            // APPNOTE 4.6.8 and https://libzip.org/specifications/extrafld.txt
            file.file_comment = String::from_utf8(
                UnicodeExtraField::try_from_reader(reader, len)?
                    .unwrap_valid(file.file_comment.as_bytes())?
                    .into_vec(),
            )?
            .into();
        }
        0x7075 => {
            // Info-ZIP Unicode Path Extra Field
            // APPNOTE 4.6.9 and https://libzip.org/specifications/extrafld.txt
            file.file_name_raw = UnicodeExtraField::try_from_reader(reader, len)?
                .unwrap_valid(&file.file_name_raw)?;
            file.file_name =
                String::from_utf8(file.file_name_raw.clone().into_vec())?.into_boxed_str();
            file.is_utf8 = true;
        }
        _ => {
            reader.read_exact(&mut vec![0u8; len as usize])?;
            // Other fields are ignored
        }
    }
    Ok(false)
}

#[cfg(test)]
mod test {
    use crate::result::ZipResult;
    use crate::unstable::read::ArchiveEntry;
    use crate::write::SimpleFileOptions;
    use crate::CompressionMethod::Stored;
    use crate::{ZipArchive, ZipWriter};
    use std::io::{Cursor, Read, Write};
    use tempdir::TempDir;

    #[test]
    fn invalid_offset() {
        use super::ZipArchive;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/invalid_offset.zip"));
        let reader = ZipArchive::new(Cursor::new(v));
        assert!(reader.is_err());
    }

    #[test]
    fn invalid_offset2() {
        use super::ZipArchive;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/invalid_offset2.zip"));
        let reader = ZipArchive::new(Cursor::new(v));
        assert!(reader.is_err());
    }

    #[test]
    fn zip64_with_leading_junk() {
        use super::ZipArchive;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/zip64_demo.zip"));
        let reader = ZipArchive::new(Cursor::new(v)).unwrap();
        assert_eq!(reader.len(), 1);
    }

    #[test]
    fn zip_contents() {
        use super::ZipArchive;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/mimetype.zip"));
        let mut reader = ZipArchive::new(Cursor::new(v)).unwrap();
        assert_eq!(reader.comment(), b"");
        assert_eq!(reader.by_index(0).unwrap().central_header_start(), 77);
    }

    #[test]
    fn zip_read_streaming() {
        use crate::unstable::read::streaming::StreamingArchive;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/mimetype.zip"));
        let reader = Cursor::new(v);
        let mut archive = StreamingArchive::new(reader);
        while archive.next_entry().unwrap().is_some() {}
    }

    #[test]
    fn zip_clone() {
        use super::ZipArchive;
        use std::io::Read;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/mimetype.zip"));
        let mut reader1 = ZipArchive::new(Cursor::new(v)).unwrap();
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

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/files_and_dirs.zip"));
        let mut zip = ZipArchive::new(Cursor::new(v)).unwrap();

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
        let files = vec![
            include_bytes!("../tests/data/zip64_magic_in_filename_1.zip").to_vec(),
            include_bytes!("../tests/data/zip64_magic_in_filename_2.zip").to_vec(),
            include_bytes!("../tests/data/zip64_magic_in_filename_3.zip").to_vec(),
            include_bytes!("../tests/data/zip64_magic_in_filename_4.zip").to_vec(),
            include_bytes!("../tests/data/zip64_magic_in_filename_5.zip").to_vec(),
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

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!(
            "../tests/data/invalid_cde_number_of_files_allocation_smaller_offset.zip"
        ));
        let reader = ZipArchive::new(Cursor::new(v));
        assert!(reader.is_err() || reader.unwrap().is_empty());
    }

    /// test case to ensure we don't preemptively over allocate based on the
    /// declared number of files in the CDE of an invalid zip when the number of
    /// files declared is less than the alleged offset in the CDE
    #[test]
    fn invalid_cde_number_of_files_allocation_greater_offset() {
        use super::ZipArchive;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!(
            "../tests/data/invalid_cde_number_of_files_allocation_greater_offset.zip"
        ));
        let reader = ZipArchive::new(Cursor::new(v));
        assert!(reader.is_err());
    }

    #[cfg(feature = "deflate64")]
    #[test]
    fn deflate64_index_out_of_bounds() -> std::io::Result<()> {
        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!(
            "../tests/data/raw_deflate64_index_out_of_bounds.zip"
        ));
        let mut reader = ZipArchive::new(Cursor::new(v))?;
        std::io::copy(&mut reader.by_index(0)?, &mut std::io::sink()).expect_err("Invalid file");
        Ok(())
    }

    #[cfg(feature = "deflate64")]
    #[test]
    fn deflate64_not_enough_space() {
        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/deflate64_issue_25.zip"));
        ZipArchive::new(Cursor::new(v)).expect_err("Invalid file");
    }

    #[cfg(feature = "_deflate-any")]
    #[test]
    fn test_read_with_data_descriptor() {
        use std::io::Read;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/data_descriptor.zip"));
        let mut reader = ZipArchive::new(Cursor::new(v)).unwrap();
        let mut decompressed = [0u8; 16];
        let mut file = reader.by_index(0).unwrap();
        assert_eq!(file.read(&mut decompressed).unwrap(), 12);
    }

    #[test]
    fn test_is_symlink() -> std::io::Result<()> {
        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/symlink.zip"));
        let mut reader = ZipArchive::new(Cursor::new(v)).unwrap();
        assert!(reader.by_index(0).unwrap().is_symlink());
        let tempdir = TempDir::new("test_is_symlink")?;
        reader.extract(&tempdir).unwrap();
        assert!(tempdir.path().join("bar").is_symlink());
        Ok(())
    }

    #[test]
    #[cfg(feature = "_deflate-any")]
    fn test_utf8_extra_field() {
        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/chinese.zip"));
        let mut reader = ZipArchive::new(Cursor::new(v)).unwrap();
        reader.by_name("七个房间.txt").unwrap();
    }

    #[test]
    fn test_utf8() {
        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/linux-7z.zip"));
        let mut reader = ZipArchive::new(Cursor::new(v)).unwrap();
        reader.by_name("你好.txt").unwrap();
    }

    #[test]
    fn test_utf8_2() {
        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/windows-7zip.zip"));
        let mut reader = ZipArchive::new(Cursor::new(v)).unwrap();
        reader.by_name("你好.txt").unwrap();
    }

    #[test]
    fn test_64k_files() -> ZipResult<()> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions {
            compression_method: Stored,
            ..Default::default()
        };
        for i in 0..=u16::MAX {
            let file_name = format!("{i}.txt");
            writer.start_file(&*file_name, options)?;
            writer.write_all(i.to_string().as_bytes())?;
        }

        let mut reader = ZipArchive::new(writer.finish()?)?;
        for i in 0..=u16::MAX {
            let expected_name = format!("{i}.txt");
            let expected_contents = i.to_string();
            let expected_contents = expected_contents.as_bytes();
            let mut file = reader.by_name(&expected_name)?;
            let mut contents = Vec::with_capacity(expected_contents.len());
            file.read_to_end(&mut contents)?;
            assert_eq!(contents, expected_contents);
            drop(file);
            contents.clear();
            let mut file = reader.by_index(i as usize)?;
            file.read_to_end(&mut contents)?;
            assert_eq!(contents, expected_contents);
        }
        Ok(())
    }
}
