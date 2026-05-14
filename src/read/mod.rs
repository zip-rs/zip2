//! Types for reading ZIP archives

use crate::compression::CompressionMethod;
use crate::cp437::FromCp437;
use crate::datetime::DateTime;
use crate::extra_fields::{ExtraField, ExtraFields};
use crate::format::flags::ZipFlags;
use crate::result::{ZipError, ZipResult, invalid};
use crate::spec::{CentralDirectoryEndInfo, DataAndPosition, FixedSizeBlock, ZipCentralEntryBlock};
use crate::types::{System, ZipFileData};
use indexmap::IndexMap;
use std::ffi::OsStr;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::OnceLock;

mod config;
pub use config::{ArchiveOffset, Config};

/// Provides high level API for reading from a stream.
pub(crate) mod stream;
pub use stream::{
    read_zipfile_from_stream, read_zipfile_from_stream_with_compressed_size,
    read_zipfile_from_stream_with_options,
};

pub(crate) mod magic_finder;
pub(crate) mod readers;

pub(crate) mod zipfile;
pub use zipfile::{ZipFile, ZipFileSeek};

pub(crate) mod zip_archive;
pub use zip_archive::{ZipArchive, ZipArchiveMetadata};

#[cfg(feature = "aes-crypto")]
pub use crate::aes::AesInfo;

pub(crate) fn make_writable_dir_all<T: AsRef<Path>>(outpath: T) -> Result<(), ZipError> {
    use std::fs;
    fs::create_dir_all(outpath.as_ref())?;
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

#[cfg(unix)]
pub(crate) fn make_symlink_impl<T>(
    outpath: &Path,
    target_str: &str,
    _existing_files: &IndexMap<Box<[u8]>, T>,
) -> ZipResult<()> {
    std::os::unix::fs::symlink(Path::new(&target_str), outpath)?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn make_symlink_impl<T>(
    outpath: &Path,
    target_str: &str,
    existing_files: &IndexMap<Box<[u8]>, T>,
) -> ZipResult<()> {
    use crate::spec::is_dir;
    let target = Path::new(OsStr::new(&target_str));
    let target_is_dir_from_archive =
        is_dir(target_str.as_bytes()) && existing_files.contains_key(target_str.as_bytes());
    let target_is_dir = if target_is_dir_from_archive {
        true
    } else if let Ok(meta) = std::fs::metadata(target) {
        meta.is_dir()
    } else {
        false
    };
    if target_is_dir {
        std::os::windows::fs::symlink_dir(target, outpath)?;
    } else {
        std::os::windows::fs::symlink_file(target, outpath)?;
    }
    Ok(())
}

#[cfg(any(windows, unix))]
pub(crate) fn make_symlink<T>(
    outpath: &Path,
    target: &[u8],
    #[cfg_attr(not(any(windows, unix)), allow(unused))] existing_files: &IndexMap<Box<[u8]>, T>,
) -> ZipResult<()> {
    let Ok(target_str) = std::str::from_utf8(target) else {
        return Err(invalid!("Invalid UTF-8 as symlink target"));
    };
    make_symlink_impl(outpath, target_str, existing_files)
}

#[cfg(not(any(windows, unix)))]
pub(crate) fn make_symlink<T>(
    outpath: &Path,
    target: &[u8],
    #[cfg_attr(not(any(windows, unix)), allow(unused))] existing_files: &IndexMap<Box<[u8]>, T>,
) -> ZipResult<()> {
    let Ok(_) = std::str::from_utf8(target) else {
        return Err(invalid!("Invalid UTF-8 as symlink target"));
    };
    use std::fs::File;
    let output = File::create(outpath);
    output?.write_all(target)?;
    Ok(())
}

#[derive(Debug)]
pub(crate) struct CentralDirectoryInfo {
    pub(crate) archive_offset: u64,
    pub(crate) directory_start: u64,
    pub(crate) number_of_files: usize,
    pub(crate) disk_number: u32,
    pub(crate) disk_with_central_directory: u32,
}

impl<'a> TryFrom<&'a CentralDirectoryEndInfo> for CentralDirectoryInfo {
    type Error = ZipError;

    fn try_from(value: &'a CentralDirectoryEndInfo) -> Result<Self, Self::Error> {
        let (relative_cd_offset, number_of_files, disk_number, disk_with_central_directory) =
            match &value.eocd64 {
                Some(DataAndPosition { data: eocd64, .. }) => {
                    if eocd64.number_of_files_on_this_disk > eocd64.number_of_files {
                        return Err(invalid!(
                            "ZIP64 footer indicates more files on this disk than in the whole archive"
                        ));
                    }
                    (
                        eocd64.central_directory_offset,
                        eocd64.number_of_files as usize,
                        eocd64.disk_number,
                        eocd64.disk_with_central_directory,
                    )
                }
                _ => (
                    u64::from(value.eocd.data.central_directory_offset),
                    value.eocd.data.number_of_files_on_this_disk as usize,
                    u32::from(value.eocd.data.disk_number),
                    u32::from(value.eocd.data.disk_with_central_directory),
                ),
            };

        let directory_start = relative_cd_offset
            .checked_add(value.archive_offset)
            .ok_or(invalid!("Invalid central directory size or offset"))?;

        Ok(Self {
            archive_offset: value.archive_offset,
            directory_start,
            number_of_files,
            disk_number,
            disk_with_central_directory,
        })
    }
}

/// Store all entries which specify a numeric "mode" which is familiar to POSIX operating systems.
#[cfg(unix)]
#[derive(Default, Debug)]
struct UnixFileModes {
    map: std::collections::BTreeMap<std::path::PathBuf, u32>,
}

#[cfg(unix)]
impl UnixFileModes {
    #[cfg_attr(not(debug_assertions), allow(unused))]
    pub fn add_mode(&mut self, path: std::path::PathBuf, mode: u32) {
        // We don't print a warning or consider it remotely out of the ordinary to receive two
        // separate modes for the same path: just take the later one.
        let old_entry = self.map.insert(path, mode);
        debug_assert_eq!(old_entry, None);
    }

    // Child nodes will be sorted later lexicographically, so reversing the order puts them first.
    pub fn all_perms_with_children_first(
        self,
    ) -> impl IntoIterator<Item = (std::path::PathBuf, std::fs::Permissions)> {
        use std::os::unix::fs::PermissionsExt;
        self.map
            .into_iter()
            .rev()
            .map(|(p, m)| (p, std::fs::Permissions::from_mode(m)))
    }
}

impl<R: Read + Seek> ZipArchive<R> {
    pub(crate) fn merge_contents<W: Write + Seek>(
        &mut self,
        mut w: W,
    ) -> ZipResult<IndexMap<Box<[u8]>, ZipFileData>> {
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
                .ok_or(invalid!(
                    "new header start from merge would have been too large"
                ))?;
            /* This is only ever used internally to cache metadata lookups (it's not part of the
             * zip spec), and 0 is the sentinel value. */
            f.central_header_start = 0;
            /* This is an atomic variable so it can be updated from another thread in the
             * implementation (which is good!). */
            if let Some(old_data_start) = f.data_start.take() {
                let new_data_start = old_data_start
                    .checked_add(first_new_file_header_start)
                    .ok_or(invalid!(
                        "new data start from merge would have been too large"
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
        /* Produce a Read that reads bytes up until the start of the central directory header.
         * This "as &mut dyn Read" trick is used elsewhere to avoid having to clone the underlying
         * handle, which it really shouldn't need to anyway. */
        let mut limited_raw = (&mut self.reader as &mut dyn Read).take(length_to_read);
        /* Copy over file data from source archive directly. */
        io::copy(&mut limited_raw, &mut w)?;

        /* Return the files we've just written to the data stream. */
        Ok(new_files)
    }

    /// Extract a Zip archive into a directory, overwriting files if they
    /// already exist. Paths are sanitized with [`ZipFile::enclosed_name`]. Symbolic links are only
    /// created and followed if the target is within the destination directory (this is checked
    /// conservatively using [`std::fs::canonicalize`]).
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
        self.extract_internal(directory, None::<fn(&Path) -> bool>)
    }

    /// Extracts a Zip archive into a directory in the same fashion as
    /// [`ZipArchive::extract`], but detects a "root" directory in the archive
    /// (a single top-level directory that contains the rest of the archive's
    /// entries) and extracts its contents directly.
    ///
    /// For a sensible default `filter`, you can use [`crate::read::root_dir_common_filter`].
    /// For a custom `filter`, see [`RootDirFilter`].
    ///
    /// See [`ZipArchive::root_dir`] for more information on how the root
    /// directory is detected and the meaning of the `filter` parameter.
    ///
    /// ## Example
    ///
    /// Imagine a Zip archive with the following structure:
    ///
    /// ```text
    /// root/file1.txt
    /// root/file2.txt
    /// root/sub/file3.txt
    /// root/sub/subsub/file4.txt
    /// ```
    ///
    /// If the archive is extracted to `foo` using [`ZipArchive::extract`],
    /// the resulting directory structure will be:
    ///
    /// ```text
    /// foo/root/file1.txt
    /// foo/root/file2.txt
    /// foo/root/sub/file3.txt
    /// foo/root/sub/subsub/file4.txt
    /// ```
    ///
    /// If the archive is extracted to `foo` using
    /// [`ZipArchive::extract_unwrapped_root_dir`], the resulting directory
    /// structure will be:
    ///
    /// ```text
    /// foo/file1.txt
    /// foo/file2.txt
    /// foo/sub/file3.txt
    /// foo/sub/subsub/file4.txt
    /// ```
    ///
    /// ## Example - No Root Directory
    ///
    /// Imagine a Zip archive with the following structure:
    ///
    /// ```text
    /// root/file1.txt
    /// root/file2.txt
    /// root/sub/file3.txt
    /// root/sub/subsub/file4.txt
    /// other/file5.txt
    /// ```
    ///
    /// Due to the presence of the `other` directory,
    /// [`ZipArchive::extract_unwrapped_root_dir`] will extract this in the same
    /// fashion as [`ZipArchive::extract`] as there is now no "root directory."
    pub fn extract_unwrapped_root_dir<P: AsRef<Path>>(
        &mut self,
        directory: P,
        root_dir_filter: impl RootDirFilter,
    ) -> ZipResult<()> {
        self.extract_internal(directory, Some(root_dir_filter))
    }

    fn extract_internal<P: AsRef<Path>>(
        &mut self,
        directory: P,
        root_dir_filter: Option<impl RootDirFilter>,
    ) -> ZipResult<()> {
        use std::fs;

        fs::create_dir_all(&directory)?;
        let directory = directory.as_ref().canonicalize()?;

        let root_dir = root_dir_filter
            .and_then(|filter| {
                self.root_dir(&filter)
                    .transpose()
                    .map(|root_dir| root_dir.map(|root_dir| (root_dir, filter)))
            })
            .transpose()?;

        // If we have a root dir, simplify the path components to be more
        // appropriate for passing to `safe_prepare_path`
        let root_dir = root_dir
            .as_ref()
            .map(|(root_dir, filter)| {
                crate::path::simplified_components(root_dir)
                    .ok_or_else(|| {
                        // Should be unreachable
                        debug_assert!(false, "Invalid root dir path");

                        invalid!("Invalid root dir path")
                    })
                    .map(|root_dir| (root_dir, filter))
            })
            .transpose()?;

        #[cfg(unix)]
        let mut files_by_unix_mode = UnixFileModes::default();

        for i in 0..self.len() {
            let mut file = self.by_index(i)?;

            let mut outpath = directory.clone();
            /* TODO: the control flow of this method call and subsequent expectations about the
             *       values in this loop is extremely difficult to follow. It also appears to
             *       perform a nested loop upon extracting every single file entry? Why does it
             *       accept two arguments that point to the same directory path, one mutable? */
            file.safe_prepare_path(directory.as_ref(), &mut outpath, root_dir.as_ref())?;

            #[cfg(any(unix, windows))]
            if file.is_symlink() {
                let mut target = Vec::with_capacity(file.size() as usize);
                file.read_to_end(&mut target)?;
                drop(file);
                make_symlink(&outpath, &target, &self.shared.files)?;
                continue;
            } else if file.is_dir() {
                make_writable_dir_all(&outpath)?;
                continue;
            }
            let mut outfile = fs::File::create(&outpath)?;
            io::copy(&mut file, &mut outfile)?;

            // Check for real permissions, which we'll set in a second pass.
            #[cfg(unix)]
            if let Some(mode) = file.unix_mode() {
                files_by_unix_mode.add_mode(outpath, mode);
            }

            // Set original timestamp.
            #[cfg(feature = "chrono")]
            if let Some(last_modified) = file.last_modified()
                && let Some(t) = last_modified.datetime_to_systemtime()
            {
                outfile.set_modified(t)?;
            }
        }

        // Ensure we update children's permissions before making a parent unwritable.
        #[cfg(unix)]
        for (path, perms) in files_by_unix_mode.all_perms_with_children_first() {
            std::fs::set_permissions(path, perms)?;
        }

        Ok(())
    }
}

/// Parse a central directory entry to collect the information for the file.
pub(crate) fn central_header_to_zip_file<R: Read + Seek>(
    reader: &mut R,
    central_directory: &CentralDirectoryInfo,
) -> ZipResult<(ZipFileData, Box<[u8]>)> {
    let central_header_start = reader.stream_position()?;

    // Parse central header
    let block = ZipCentralEntryBlock::parse(reader)?;

    let (file, file_name_raw) = central_header_to_zip_file_inner(
        reader,
        central_directory.archive_offset,
        central_header_start,
        block,
    )?;

    let central_header_end = reader.stream_position()?;

    reader.seek(SeekFrom::Start(central_header_end))?;
    Ok((file, file_name_raw.into()))
}

#[inline]
fn read_variable_length_byte_field<R: Read>(reader: &mut R, len: usize) -> ZipResult<Vec<u8>> {
    let mut data = vec![0; len];
    if let Err(e) = reader.read_exact(&mut data) {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            return Err(invalid!(
                "Variable-length field extends beyond file boundary"
            ));
        }
        return Err(e.into());
    }
    Ok(data)
}

/// Parse a central directory entry to collect the information for the file.
fn central_header_to_zip_file_inner<R: Read>(
    reader: &mut R,
    archive_offset: u64,
    central_header_start: u64,
    block: ZipCentralEntryBlock,
) -> ZipResult<(ZipFileData, Vec<u8>)> {
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

    let is_utf8 = ZipFlags::matching(flags, ZipFlags::LanguageEncoding);

    let mut file_name_raw = read_variable_length_byte_field(reader, file_name_length as usize)?;
    let extra_fields_raw = read_variable_length_byte_field(reader, extra_field_length as usize)?;
    eprintln!("EXTRA FIELDS RAW LEN: {}", extra_fields_raw.len());
    let file_comment_raw = read_variable_length_byte_field(reader, file_comment_length as usize)?;
    let file_comment: Box<str> = if is_utf8 {
        String::from_utf8_lossy(&file_comment_raw).into()
    } else {
        file_comment_raw.from_cp437()?.into()
    };

    let (version_made_by, system) = System::extract_bytes(version_made_by);
    let extra_fields = ExtraFields::parse(&extra_fields_raw, &block)?;
    // Construct the result
    let mut result = ZipFileData {
        system,
        version_made_by,
        compression_method: CompressionMethod::parse_from_u16(compression_method),
        last_modified_time: DateTime::try_from_msdos(last_mod_date, last_mod_time).ok(),
        crc32,
        compressed_size: compressed_size.into(),
        uncompressed_size: uncompressed_size.into(),
        flags,
        file_comment,
        header_start: offset.into(),
        extra_data_start: None,
        central_header_start,
        data_start: OnceLock::new(),
        external_attributes: external_file_attributes,
        large_file: false,
        aes_mode: None,
        extra_fields,
    };
    result.apply_extra_fields(&mut file_name_raw)?;

    // Account for shifted zip offsets.
    result.header_start = result
        .header_start
        .checked_add(archive_offset)
        .ok_or(invalid!("Archive header is too large"))?;

    Ok((result, file_name_raw))
}

/// A trait for exposing file metadata inside the zip.
pub trait HasZipMetadata {
    /// Get the file metadata
    fn get_metadata(&self) -> &ZipFileData;
}

/// Options for reading a file from an archive.
#[derive(Default)]
#[non_exhaustive]
pub struct ZipReadOptions<'a> {
    /// The password to use when decrypting the file.  This is ignored if not required.
    password: Option<&'a [u8]>,

    /// Ignore the value of the encryption flag and proceed as if the file were plaintext.
    ignore_encryption_flag: bool,

    /// Ignore the crc32 of the file
    ignore_crc: bool,
    /// override the compressed_size for stream read
    force_compressed_size: Option<u64>,
    /// override the uncompressed_size for stream read
    force_uncompressed_size: Option<u64>,
    /// override the checksum for stream read
    force_crc: Option<u32>,
}

impl<'a> ZipReadOptions<'a> {
    /// Create a new set of options with the default values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the password, if any, to use.  Return for chaining.
    #[must_use]
    pub fn password(mut self, password: Option<&'a [u8]>) -> Self {
        self.password = password;
        self
    }

    /// Set the ignore encryption flag.  Return for chaining.
    #[must_use]
    pub fn ignore_encryption_flag(mut self, ignore: bool) -> Self {
        self.ignore_encryption_flag = ignore;
        self
    }

    /// Ignore the CRC32 of the file
    #[must_use]
    pub fn ignore_crc32(mut self, should_ignore: bool) -> Self {
        self.ignore_crc = should_ignore;
        self
    }

    /// Override the compressed_size
    #[must_use]
    pub fn override_compressed_size(mut self, comp_size: u64) -> Self {
        self.force_compressed_size = Some(comp_size);
        self
    }

    /// Override the uncompressed_size
    #[must_use]
    pub fn override_uncompressed_size(mut self, uncomp_size: u64) -> Self {
        self.force_uncompressed_size = Some(uncomp_size);
        self
    }

    /// Override the checksum
    #[must_use]
    pub fn override_crc(mut self, crc: u32) -> Self {
        self.force_crc = Some(crc);
        self
    }
}

/// A filter that determines whether an entry should be ignored when searching
/// for the root directory of a Zip archive.
///
/// Returns `true` if the entry should be considered, and `false` if it should
/// be ignored.
///
/// See [`root_dir_common_filter`] for a sensible default filter.
pub trait RootDirFilter: Fn(&Path) -> bool {}
impl<F: Fn(&Path) -> bool> RootDirFilter for F {}

/// Common filters when finding the root directory of a Zip archive.
///
/// This filter is a sensible default for most use cases and filters out common
/// system files that are usually irrelevant to the contents of the archive.
///
/// Currently, the filter ignores:
/// - `/__MACOSX/`
/// - `/.DS_Store`
/// - `/Thumbs.db`
///
/// **This function is not guaranteed to be stable and may change in future versions.**
///
/// # Example
///
/// ```rust
/// # use std::path::Path;
/// assert!(zip::read::root_dir_common_filter(Path::new("foo.txt")));
/// assert!(!zip::read::root_dir_common_filter(Path::new(".DS_Store")));
/// assert!(!zip::read::root_dir_common_filter(Path::new("Thumbs.db")));
/// assert!(!zip::read::root_dir_common_filter(Path::new("__MACOSX")));
/// assert!(!zip::read::root_dir_common_filter(Path::new("__MACOSX/foo.txt")));
/// ```
#[must_use]
pub fn root_dir_common_filter(path: &Path) -> bool {
    const COMMON_FILTER_ROOT_FILES: &[&str] = &[".DS_Store", "Thumbs.db"];

    if path.starts_with("__MACOSX") {
        return false;
    }

    if path.components().count() == 1
        && path.file_name().is_some_and(|file_name| {
            COMMON_FILTER_ROOT_FILES
                .iter()
                .map(OsStr::new)
                .any(|cmp| cmp == file_name)
        })
    {
        return false;
    }

    true
}
