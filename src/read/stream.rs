//! Code related to stream reading

use crate::ZipReadOptions;
use crate::extra_fields::ExtraFields;
use crate::format::blocks::{FixedSizeBlock, Pod, ZipCentralEntryBlock, ZipLocalEntryBlock};
use crate::format::magic::Magic;
#[cfg(feature = "extract")]
use crate::read::SymlinkExtractAction;
#[cfg(feature = "extract")]
use crate::read::extract::{MAX_SYMLINK_TARGET_LEN, make_symlink_as_file};
#[cfg(feature = "extract")]
use crate::read::extract::{make_symlink, make_symlink_as_file_utf8};
use crate::read::{
    HeaderConsistencyMetadata, ZipFile, ZipFileData, ZipFileEntry, ZipResult,
    central_header_to_zip_file_inner,
};
use crate::result::{ZipError, invalid};

use crate::read::stream::ReadFromStreamResult::{
    CentralDirectoryHeader, EndOfCentralDirectory, File,
};
#[cfg(feature = "extract")]
use indexmap::IndexMap;
use std::borrow::Cow;
use std::io::{self, Read};
#[cfg(feature = "extract")]
use std::path::{Path, PathBuf};

/// Stream decoder for zip.
#[derive(Debug)]
pub struct ZipStreamReader<R> {
    reader: R,
    enforce_header_consistency: bool,
}

impl<R> ZipStreamReader<R> {
    /// Create a new `ZipStreamReader` with default settings. For backward-compatibility,
    /// this doesn't check that the local headers match the central directory.
    pub const fn new(reader: R) -> Self {
        Self {
            reader,
            enforce_header_consistency: false,
        }
    }

    /// Configure whether to validate consistency between Local File Headers (LFH)
    /// and Central Directory File Headers (CDFH).
    ///
    /// When enabled, any discrepancy (such as Zip Cloaking / Dual-Header Smuggling attacks
    /// where an attacker provides different names or hidden files across headers) will cause
    /// the extraction to abort with [`ZipError::InvalidArchive`].
    ///
    /// The headers can only be compared once the central directory, at the end of the stream,
    /// has been read. [`ZipStreamReader::extract`] therefore extracts into a temporary directory
    /// inside the destination and moves the entries into place only if the check passes. A
    /// [`ZipStreamVisitor`] passed to [`ZipStreamReader::visit`] sees every entry before the
    /// check, so it must not act on them until `visit` returns `Ok`.
    #[must_use]
    pub const fn enforce_header_consistency(mut self, enforce: bool) -> Self {
        self.enforce_header_consistency = enforce;
        self
    }
}

fn parse_central_directory<R: Read>(reader: &mut R) -> ZipResult<ZipFileEntry<'static>> {
    // Give archive_offset and central_header_start dummy value 0, since
    // they are not used in the output.
    let archive_offset = 0;
    let central_header_start = 0;

    // Parse central header
    let block = ZipCentralEntryBlock::parse(reader)?;
    let (file, file_name_raw) =
        central_header_to_zip_file_inner(reader, archive_offset, central_header_start, block)?;
    Ok(ZipFileEntry {
        file_name_raw: Cow::Owned(file_name_raw),
        data: Cow::Owned(file),
    })
}

impl<R: Read> ZipStreamReader<R> {
    /// Iterate over the stream and extract all file and their
    /// metadata.
    pub fn visit<V: ZipStreamVisitor>(self, visitor: &mut V) -> ZipResult<()> {
        struct CountingReader<R> {
            reader: R,
            count: u64,
        }

        impl<R: Read> Read for CountingReader<R> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = self.reader.read(buf)?;
                self.count += n as u64;
                Ok(n)
            }
        }

        struct LocalEntryMetadata {
            metadata: HeaderConsistencyMetadata<Box<[u8]>>,
            stream_offset: u64,
        }

        struct CentralEntryMetadata {
            metadata: HeaderConsistencyMetadata<Box<[u8]>>,
            header_start: u64,
        }

        let mut reader = CountingReader {
            reader: self.reader,
            count: 0,
        };

        let mut local_entries = Vec::new();

        let has_central_directory = loop {
            let offset_before_read = reader.count;
            let read_from_stream_result =
                read_zipfile_from_stream_with_options_internal(&mut reader, Default::default())?;
            match read_from_stream_result {
                File(mut file) => {
                    if self.enforce_header_consistency {
                        local_entries.push(LocalEntryMetadata {
                            metadata: file
                                .data
                                .consistency_metadata(file.name_raw(), true)
                                .into_owned(),
                            stream_offset: offset_before_read,
                        });
                    }
                    visitor.visit_file(&mut file)?;
                }
                CentralDirectoryHeader => break true,
                EndOfCentralDirectory => break false,
            }
        };

        let mut central_entries = Vec::new();

        if has_central_directory {
            let signature = Magic::CENTRAL_DIRECTORY_HEADER_SIGNATURE.to_le_bytes();
            let mut central_directory = (&signature[..]).chain(&mut reader);
            while let Ok(metadata) = parse_central_directory(&mut central_directory) {
                if self.enforce_header_consistency {
                    central_entries.push(CentralEntryMetadata {
                        metadata: metadata
                            .data
                            .consistency_metadata(metadata.name_raw(), false)
                            .into_owned(),
                        header_start: metadata.header_start(),
                    });
                }
                visitor.visit_additional_metadata(&metadata)?;
            }
        }

        if self.enforce_header_consistency {
            if local_entries.len() != central_entries.len() {
                return Err(invalid!(
                    "header inconsistency detected: entry count mismatch ({} local vs {} central)",
                    local_entries.len(),
                    central_entries.len()
                ));
            }

            // Sort central directory entries by their local header offset (header_start)
            // to align with the sequential physical stream order of the local headers.
            central_entries.sort_by_key(|e| e.header_start);

            let base_offset = central_entries.first().map_or(0, |e| e.header_start);
            let mut seen_names = std::collections::HashSet::with_capacity(central_entries.len());

            for (i, (local, central)) in
                local_entries.iter().zip(central_entries.iter()).enumerate()
            {
                // Ensure strictly monotonic local header offsets in central directory
                if i > 0 && central.header_start <= central_entries[i - 1].header_start {
                    return Err(invalid!(
                        "header inconsistency detected: two central directory entries for same file"
                    ));
                }

                // Verify that the local header offset matches the physical stream offset exactly
                let expected_offset =
                    base_offset
                        .checked_add(local.stream_offset)
                        .ok_or(invalid!(
                            "header inconsistency detected: local header offset overflow"
                        ))?;
                if central.header_start != expected_offset {
                    return Err(invalid!(
                        "header inconsistency detected: local header offset mismatch for '{}' (expected {}, central claims {})",
                        String::from_utf8_lossy(&local.metadata.name),
                        expected_offset,
                        central.header_start
                    ));
                }

                local.metadata.validate_matches(&central.metadata)?;

                if !seen_names.insert(&central.metadata.name) {
                    return Err(invalid!(
                        "header inconsistency detected: duplicate file name '{}'",
                        String::from_utf8_lossy(&central.metadata.name)
                    ));
                }
            }
        }

        Ok(())
    }

    /// Extract a Zip archive into a directory, overwriting files if they
    /// already exist. Paths are sanitized with [`ZipFile::enclosed_name`].
    ///
    /// Extraction is not atomic; If an error is encountered, some of the files
    /// may be left on disk. The file(s) and dir(s) are first created, then the
    /// permissions are applied to them
    ///
    /// Extraction of symlink is not possible since we don't have access to the
    /// external attributes in the local headers of the entries
    #[cfg(feature = "extract")]
    pub fn extract<P: AsRef<Path>>(self, directory: P) -> ZipResult<()> {
        /// Destination, every entry written in this pass (raw local name -> the checked output
        /// path `visit_file` wrote it to), and the modes the central directory gives them.
        struct Extractor {
            outpath: PathBuf,
            resolved_paths: IndexMap<Box<[u8]>, PathBuf>,
            file_permissions: std::collections::BTreeMap<PathBuf, u32>,
        }
        impl ZipStreamVisitor for Extractor {
            fn visit_file<R: Read>(&mut self, file: &mut ZipFile<'_, R>) -> ZipResult<()> {
                let extract_options = crate::read::extract::ExtractOptions::default();
                let mut outpath = self.outpath.clone();
                file.safe_prepare_path(
                    &self.outpath,
                    &mut outpath,
                    None::<&(_, fn(&Path) -> bool)>,
                )?;
                self.resolved_paths
                    .insert(file.name_raw().into(), outpath.clone());

                if file.is_symlink() {
                    // Same bound as `ZipArchive::extract`: the declared
                    // uncompressed size is attacker-controlled, so reject
                    // oversized entries rather than pre-allocating them.
                    //
                    // `is_symlink()` is currently always false on this path
                    // (`from_local_block` has no external attributes to read),
                    // so this is defence in depth -- see
                    // `stream_does_not_expose_the_symlink_bit`.
                    let declared_len = file.size();
                    if declared_len > MAX_SYMLINK_TARGET_LEN {
                        return Err(invalid!(
                            "symlink target declares {} bytes, more than the {} byte limit",
                            declared_len,
                            MAX_SYMLINK_TARGET_LEN
                        ));
                    }

                    let mut target = Vec::with_capacity(declared_len as usize);
                    file.read_to_end(&mut target)?;
                    match extract_options.symlink_action {
                        SymlinkExtractAction::ExtractAsFile => {
                            make_symlink_as_file(&outpath, &target)?;
                        }
                        SymlinkExtractAction::ExtractAsFileUtf8 => {
                            make_symlink_as_file_utf8(&outpath, &target)?;
                        }
                        _ => {
                            make_symlink(
                                &self.outpath,
                                &outpath,
                                &target,
                                &self.resolved_paths,
                                extract_options.symlink_action,
                            )?;
                        }
                    }
                    return Ok(());
                }
                if file.is_dir() {
                    fs::create_dir_all(&outpath)?;
                } else {
                    let mut outfile = fs::File::create(&outpath)?;
                    io::copy(file, &mut outfile)?;
                }

                Ok(())
            }

            fn visit_additional_metadata(&mut self, metadata: &ZipFileEntry<'_>) -> ZipResult<()> {
                // The central directory is not checked against the local headers, so its
                // names only select entries this pass already wrote: a name that wasn't
                // written gets no mode, and the mode goes to the path `visit_file` checked,
                // never to a path rebuilt from the central directory.
                if let (Some(outpath), Some(mode)) = (
                    self.resolved_paths.get(metadata.name_raw()),
                    metadata.unix_mode(),
                ) {
                    self.file_permissions.insert(outpath.clone(), mode);
                }
                Ok(())
            }
        }
        /// Moves the entries `visit` wrote under `staging` to the same place under `dest`,
        /// resolving each path again as `visit_file` does.
        fn move_staged_entries(
            staging: &mut PathBuf,
            dest: &Path,
            extractor: &mut Extractor,
        ) -> ZipResult<()> {
            // An entry named like `staging` would be moved into it, so first rename `staging`
            // to a name no entry has. Ask the filesystem, since it may ignore case.
            let written_to = staging.clone();
            let has_entry_named_like = |dir: &Path| {
                dir.file_name()
                    .is_some_and(|name| dir.join(name).symlink_metadata().is_ok())
            };
            while has_entry_named_like(staging) {
                let renamed = staging_path(dest)?;
                fs::rename(&*staging, &renamed)?;
                *staging = renamed;
            }

            for staged in extractor.resolved_paths.values_mut() {
                let relative = staged
                    .strip_prefix(&written_to)
                    .map_err(|_| invalid!("Entry was not extracted to the staging directory"))?;
                let from = staging.join(relative);
                let components = relative.iter().map(|c| c.to_os_string());
                let outpath = crate::path::resolve_enclosed(dest, dest, components, true)?;
                if outpath.starts_with(&*staging) {
                    return Err(invalid!("Entry would be written to the staging directory"));
                }
                if fs::symlink_metadata(&from)?.is_dir() {
                    fs::create_dir_all(&outpath)?;
                } else if let Err(e) = fs::rename(&from, &outpath) {
                    if e.kind() != io::ErrorKind::CrossesDevices {
                        return Err(e.into());
                    }
                    fs::copy(&from, &outpath)?;
                }
                if let Some(mode) = extractor.file_permissions.remove(&*staged) {
                    extractor.file_permissions.insert(outpath.clone(), mode);
                }
                *staged = outpath;
            }
            Ok(())
        }

        use std::fs;
        fs::create_dir_all(&directory)?;
        let directory = directory.as_ref().canonicalize()?;

        // The headers are only checked after the last entry, so write nothing to `directory`
        // before that.
        let staging = if self.enforce_header_consistency {
            Some(create_staging_dir(&directory)?)
        } else {
            None
        };

        let mut extractor = Extractor {
            outpath: staging.clone().unwrap_or_else(|| directory.clone()),
            resolved_paths: IndexMap::new(),
            file_permissions: Default::default(),
        };
        let visited = self.visit(&mut extractor);
        if let Some(mut staging) = staging {
            let moved = visited
                .and_then(|()| move_staged_entries(&mut staging, &directory, &mut extractor));
            let removed = fs::remove_dir_all(&staging);
            moved?;
            removed?;
        } else {
            visited?;
        }

        // As `ZipArchive::extract`: children before parents, so a read-only directory does not
        // block setting its contents. Never through a symlink: `set_permissions` follows them.
        // Off Unix, only the read-only flag applies.
        for (path, mode) in extractor.file_permissions.into_iter().rev() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                crate::read::extract::set_permissions_no_follow_symlink(
                    &path,
                    std::fs::Permissions::from_mode(mode),
                )?;
            }
            #[cfg(not(unix))]
            if mode & 0o222 == 0 {
                let metadata = fs::symlink_metadata(&path)?;
                if !metadata.is_symlink() {
                    let mut permissions = metadata.permissions();
                    permissions.set_readonly(true);
                    fs::set_permissions(&path, permissions)?;
                }
            }
        }
        Ok(())
    }
}

/// A path in `dest` with a random name.
#[cfg(feature = "extract")]
fn staging_path(dest: &Path) -> io::Result<PathBuf> {
    let n = getrandom::u64().map_err(io::Error::other)?;
    Ok(dest.join(format!(".zip-extract-{n:016x}")))
}

/// Creates a new, empty directory with a random name in `dest`.
#[cfg(feature = "extract")]
fn create_staging_dir(dest: &Path) -> io::Result<PathBuf> {
    loop {
        let staging = staging_path(dest)?;
        match std::fs::create_dir(&staging) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            result => return result.map(|()| staging),
        }
    }
}

/// Visitor for `ZipStreamReader`
pub trait ZipStreamVisitor {
    ///  * `file` - contains the content of the file and most of the metadata,
    ///    except:
    ///     - `comment`: set to an empty string
    ///     - `data_start`: set to 0
    ///     - `external_attributes`: `unix_mode()`: will return None
    fn visit_file<R: Read>(&mut self, file: &mut ZipFile<'_, R>) -> ZipResult<()>;

    /// This function is guaranteed to be called after all `visit_file`s.
    ///
    ///  * `metadata` - Provides missing metadata in `visit_file`.
    fn visit_additional_metadata(&mut self, metadata: &ZipFileEntry<'_>) -> ZipResult<()>;
}

/// Read `ZipFile` structures from a non-seekable reader.
///
/// This is an alternative method to read a zip file. If possible, use the `ZipArchive` functions
/// as some information will be missing when reading this manner.
///
/// Reads a file header from the start of the stream. Will return `Ok(Some(..))` if a file is
/// present at the start of the stream. Returns `Ok(None)` if the start of the central directory
/// is encountered. No more files should be read after this.
///
/// The Drop implementation of `ZipFile` ensures that the reader will be correctly positioned after
/// the structure is done.
///
/// Missing fields are:
/// * `comment`: set to an empty string
/// * `data_start`: set to 0
/// * `external_attributes`: `unix_mode()`: will return None
pub fn read_zipfile_from_stream<R: Read>(reader: &mut R) -> ZipResult<Option<ZipFile<'_, R>>> {
    read_zipfile_from_stream_with_options(reader, ZipReadOptions::default())
}

/// Read `ZipFile` from a non-seekable reader like [`read_zipfile_from_stream`] does, but assume the
/// given compressed size and don't read any further ahead than that.
pub fn read_zipfile_from_stream_with_compressed_size<R: Read>(
    reader: &mut R,
    compressed_size: u64,
) -> ZipResult<Option<ZipFile<'_, R>>> {
    let options = ZipReadOptions::default().override_compressed_size(compressed_size);
    read_zipfile_from_stream_with_options(reader, options)
}

/// Same as `read_zipfile_from_stream` but with `ZipReadOptions`
/// Since LZMA decoding requires the uncompressed length, you will need to override it
pub fn read_zipfile_from_stream_with_options<'a, R: Read>(
    reader: &'a mut R,
    options: ZipReadOptions<'a>,
) -> ZipResult<Option<ZipFile<'a, R>>> {
    let result = read_zipfile_from_stream_with_options_internal(reader, options)?;
    match result {
        ReadFromStreamResult::File(file) => Ok(Some(file)),
        ReadFromStreamResult::CentralDirectoryHeader => Ok(None),
        ReadFromStreamResult::EndOfCentralDirectory => Ok(None),
    }
}

enum ReadFromStreamResult<'a, R: Read> {
    File(ZipFile<'a, R>),
    CentralDirectoryHeader,
    EndOfCentralDirectory,
}

/// Returns Ok(Ok(file)) if a file was read, Ok(Err(true)) if a central directory entry signature was found,
/// and Err(false) if an EOCD signature was found.
fn read_zipfile_from_stream_with_options_internal<'a, R: Read>(
    reader: &'a mut R,
    mut options: ZipReadOptions<'a>,
) -> ZipResult<ReadFromStreamResult<'a, R>> {
    // We can't use the typical [`ZipLocalEntryBlock::parse`] method, as we follow separate code paths depending on the
    // "magic" value (since the magic value will be from the central directory header if we've
    // finished iterating over all the actual files).
    let mut magic_buf = [0; size_of::<u32>()];
    reader.read_exact(&mut magic_buf)?;

    match Magic::from_le_bytes(magic_buf) {
        Magic::LOCAL_FILE_HEADER_SIGNATURE => (),
        Magic::CENTRAL_DIRECTORY_HEADER_SIGNATURE => return Ok(CentralDirectoryHeader),
        Magic::CENTRAL_DIRECTORY_END_SIGNATURE => return Ok(EndOfCentralDirectory), // Empty archive may have no CENTRAL_DIRECTORY_HEADER_SIGNATURE
        _ => return Err(ZipLocalEntryBlock::WRONG_MAGIC_ERROR),
    }

    let mut block = ZipLocalEntryBlock::zeroed();
    reader.read_exact(block.as_bytes_mut())?;

    let block = block.from_le();
    // parse file_name_raw
    let file_name_length: usize = block.file_name_length.into();
    let mut file_name_raw = vec![0u8; file_name_length];
    if let Err(e) = reader.read_exact(&mut file_name_raw) {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            return Err(invalid!("File name extends beyond file boundary"));
        }
        return Err(e.into());
    }

    // parse extra fields raw
    let extra_field_length: usize = block.extra_field_length.into();
    let mut extra_fields_raw = vec![0u8; extra_field_length];
    if let Err(e) = reader.read_exact(&mut extra_fields_raw) {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            return Err(invalid!("Extra field extends beyond file boundary"));
        }
        return Err(e.into());
    }
    // parse extra fields
    let extra_fields = ExtraFields::parse(&extra_fields_raw, &block, true)?;
    let mut data = ZipFileData::from_local_block(block, extra_fields)?;
    data.apply_extra_fields(&mut file_name_raw)?;
    if data.is_using_data_descriptor() {
        if let Some(comp_size) = options.force_compressed_size {
            data.compressed_size = comp_size;
        } else {
            return Err(ZipError::UnsupportedArchive(
                "The file length is not available in the local header",
            ));
        }
    }
    if let Some(uncomp_size) = options.force_uncompressed_size {
        data.uncompressed_size = uncomp_size;
    } else if data.is_using_data_descriptor() {
        // The local header doesn't carry the size; it follows the data.
        options.size_unknown = true;
    }
    if let Some(crc) = options.force_crc {
        data.crc32 = crc;
    }

    let limit_reader = reader.take(data.compressed_size);
    let reader = data.make_reader(limit_reader, options)?;
    Ok(File(ZipFile {
        file_name_raw: Cow::Owned(file_name_raw),
        data: Cow::Owned(data),
        reader,
    }))
}

#[cfg(test)]
mod tests {
    use crate::ZipWriter;
    use crate::read::ZipFile;
    use crate::read::ZipFileEntry;
    use crate::read::stream::{ZipStreamReader, ZipStreamVisitor};
    use crate::result::ZipResult;
    use std::collections::BTreeSet;
    use std::io::{Cursor, Read};

    #[test]
    fn zip_read_streaming_visitor() {
        let reader =
            ZipStreamReader::new(Cursor::new(include_bytes!("../../tests/data/mimetype.zip")));

        #[derive(Default)]
        struct V {
            filenames: BTreeSet<Box<str>>,
        }
        impl ZipStreamVisitor for V {
            fn visit_file<R: Read>(&mut self, file: &mut ZipFile<'_, R>) -> ZipResult<()> {
                if file.is_file() {
                    let file_name = file.name().unwrap();
                    self.filenames.insert(file_name.into());
                }

                Ok(())
            }
            fn visit_additional_metadata(
                &mut self,
                zip_file_metadata: &ZipFileEntry<'_>,
            ) -> ZipResult<()> {
                if zip_file_metadata.is_file() {
                    let file_name = zip_file_metadata.name().unwrap();
                    assert!(
                        self.filenames.contains::<str>(file_name.as_ref()),
                        "{} is missing its file content",
                        file_name
                    );
                }

                Ok(())
            }
        }

        reader.visit(&mut V::default()).unwrap();
    }

    #[test]
    fn file_and_dir_predicates() {
        let reader = ZipStreamReader::new(Cursor::new(include_bytes!(
            "../../tests/data/files_and_dirs.zip"
        )));

        #[derive(Default)]
        struct V {
            filenames: BTreeSet<Box<str>>,
        }
        impl ZipStreamVisitor for V {
            fn visit_file<R: Read>(&mut self, file: &mut ZipFile<'_, R>) -> ZipResult<()> {
                let full_name = file.enclosed_name().unwrap();
                let file_name = full_name.file_name().unwrap().to_str().unwrap();
                assert!(
                    (file_name.starts_with("dir") && file.is_dir())
                        || (file_name.starts_with("file") && file.is_file())
                );

                if file.is_file() {
                    let file_name = file.name().unwrap();
                    self.filenames.insert(file_name.into());
                }

                Ok(())
            }
            fn visit_additional_metadata(
                &mut self,
                zip_file_metadata: &ZipFileEntry<'_>,
            ) -> ZipResult<()> {
                if zip_file_metadata.is_file() {
                    let file_name = zip_file_metadata.name().unwrap();
                    assert!(
                        self.filenames.contains::<str>(file_name.as_ref()),
                        "{} is missing its file content",
                        file_name
                    );
                }

                Ok(())
            }
        }

        reader.visit(&mut V::default()).unwrap();
    }

    /// Symlinks being extracted shouldn't be followed out of the destination directory.
    /// Cannot use fs with miri CI
    #[cfg(all(not(miri), feature = "extract"))]
    #[test]
    fn test_cannot_symlink_outside_destination() -> ZipResult<()> {
        use crate::ZipWriter;
        use crate::write::SimpleFileOptions;
        use std::fs::create_dir;
        use tempfile::TempDir;

        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.add_symlink("symlink/", "../dest-sibling/", SimpleFileOptions::default())?;
        writer.start_file("symlink/dest-file", SimpleFileOptions::default())?;
        let reader = ZipStreamReader::new(writer.finish()?);
        let dest_parent = TempDir::with_prefix("stream__cannot_symlink_outside_destination")?;
        let dest_sibling = dest_parent.path().join("dest-sibling");
        create_dir(&dest_sibling)?;
        let dest = dest_parent.path().join("dest");
        create_dir(&dest)?;
        assert!(reader.extract(dest).is_err());
        assert!(!dest_sibling.join("dest-file").exists());
        Ok(())
    }

    /// The stream reader builds each entry from the *local* file header, which
    /// has no external-attributes field (`ZipFileData::from_local_block` sets it
    /// to 0), so `is_symlink()` is always false on this path and the symlink
    /// branch in `extract` is currently unreachable.
    ///
    /// The `MAX_SYMLINK_TARGET_LEN` guard is still kept there, for consistency
    /// with `ZipArchive::extract` and so it is already in place if that ever
    /// changes. This test pins the current behaviour: if the local header starts
    /// carrying the attribute, the assertion fails and the guard needs a real
    /// regression test.
    #[test]
    fn stream_does_not_expose_the_symlink_bit() -> ZipResult<()> {
        use crate::ZipWriter;
        use crate::write::SimpleFileOptions;

        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.add_symlink("link", "/target", SimpleFileOptions::default())?;
        let mut cur = writer.finish()?;
        cur.set_position(0);

        let file = crate::read::read_zipfile_from_stream(&mut cur)?.unwrap();
        assert!(
            !file.is_symlink(),
            "the stream path now sees the symlink bit -- the MAX_SYMLINK_TARGET_LEN \
             guard in `stream.rs` is reachable and needs a regression test"
        );

        Ok(())
    }

    /// Cannot use fs with miri CI
    #[cfg(all(not(miri), feature = "extract"))]
    #[test]
    fn test_can_create_destination() -> ZipResult<()> {
        use tempfile::TempDir;

        let v = include_bytes!("../../tests/data/mimetype.zip");
        let reader = ZipStreamReader::new(v.as_ref());
        let dest = TempDir::with_prefix("stream_test_can_create_destination").unwrap();
        reader.extract(&dest)?;
        assert!(dest.path().join("mimetype").exists());
        Ok(())
    }

    #[test]
    fn zip_read_streaming() {
        use super::read_zipfile_from_stream;

        let mut reader = Cursor::new(include_bytes!("../../tests/data/mimetype.zip"));
        loop {
            if read_zipfile_from_stream(&mut reader).unwrap().is_none() {
                break;
            }
        }
    }

    #[test]
    #[cfg(feature = "deflate")]
    fn zip_read_streaming_compressed() {
        use super::read_zipfile_from_stream_with_compressed_size;
        use crate::ZipWriter;
        use crate::write::SimpleFileOptions;
        use std::io::Write;

        let compression_method = crate::CompressionMethod::Deflated;
        let options = SimpleFileOptions::default()
            .compression_method(compression_method)
            .unix_permissions(0o755);

        let mut bytes = Vec::new();
        let mut writer = ZipWriter::new(std::io::Cursor::new(&mut bytes));
        writer.start_file("file.txt", options).unwrap();
        write!(&mut writer, "{}", "test-".repeat(100)).unwrap();
        writer.finish().unwrap();

        let compressed_size = u32::from_le_bytes(bytes[18..22].try_into().unwrap());
        let uncompressed_size = u32::from_le_bytes(bytes[22..26].try_into().unwrap());

        assert_eq!(compressed_size, 14);
        assert_eq!(uncompressed_size as usize, "test-".len() * 100);

        let mut reader = Cursor::new(bytes);
        loop {
            if read_zipfile_from_stream_with_compressed_size(
                &mut reader,
                u64::from(compressed_size),
            )
            .unwrap()
            .is_none()
            {
                break;
            }
        }
    }

    #[test]
    #[cfg(feature = "aes-crypto")]
    fn zip_read_streaming_compressed_and_aes() {
        use super::read_zipfile_from_stream_with_options;
        use crate::ZipReadOptions;

        let bytes = include_bytes!("../../tests/data/aes_archive.zip");
        let compressed_size = 46;

        let mut reader = Cursor::new(bytes);
        const PASSWORD: &[u8] = b"helloworld";
        let options = ZipReadOptions::new()
            .password(Some(PASSWORD))
            .override_compressed_size(compressed_size);

        // we simulate the fact that we need the compressed size like a streamed zip
        let result = read_zipfile_from_stream_with_options(&mut reader, options);
        let optional_file = result.unwrap();
        let mut file = optional_file.unwrap();

        let file_name = file.name().unwrap();
        assert_eq!(file_name, "secret_data_128");

        const SECRET_CONTENT: &str = "Lorem ipsum dolor sit amet";
        let mut decrypted_content = String::new();
        file.read_to_string(&mut decrypted_content)
            .expect("couldn't read encrypted file");
        assert_eq!(SECRET_CONTENT, decrypted_content);
    }

    #[test]
    #[cfg(feature = "aes-crypto")]
    fn zip_read_streaming_compressed_and_aes_without_size() {
        use super::read_zipfile_from_stream_with_options;
        use crate::ZipReadOptions;

        let bytes = include_bytes!("../../tests/data/aes_archive.zip");

        let mut reader = Cursor::new(bytes);
        const PASSWORD: &[u8] = b"helloworld";
        let options = ZipReadOptions::new().password(Some(PASSWORD));

        // the zip already has the compressed size (it's not a streamed zip)
        let result = read_zipfile_from_stream_with_options(&mut reader, options);
        let optional_file = result.unwrap();
        let mut file = optional_file.unwrap();

        let file_name = file.name().unwrap();
        assert_eq!(file_name, "secret_data_128");

        const SECRET_CONTENT: &str = "Lorem ipsum dolor sit amet";
        let mut decrypted_content = String::new();
        file.read_to_string(&mut decrypted_content)
            .expect("couldn't read encrypted file");
        assert_eq!(SECRET_CONTENT, decrypted_content);
    }

    #[test]
    fn zip_read_streaming_zipwriter() {
        use crate::CompressionMethod;
        use crate::ZipReadOptions;
        use crate::ZipWriter;
        use crate::read::read_zipfile_from_stream;
        use crate::read::read_zipfile_from_stream_with_compressed_size;
        use crate::read::read_zipfile_from_stream_with_options;
        use crate::write::SimpleFileOptions;
        use std::io::Write;

        let mut buffer = Vec::new();
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let mut archive = ZipWriter::new_stream(Cursor::new(&mut buffer));
        archive.start_file("name", options).unwrap();
        archive.write_all(b"test").unwrap();
        let compressed_size = b"test".len() as u64;
        archive.finish().unwrap();
        {
            // reading will fail because it's a streamed zipfile and we don't know the size
            let mut reader = Cursor::new(buffer.clone());
            let result = read_zipfile_from_stream(&mut reader);
            assert!(result.is_err());
        }
        {
            // reading the file will fail because of the invalid checksum
            let mut reader = Cursor::new(buffer.clone());
            let result =
                read_zipfile_from_stream_with_compressed_size(&mut reader, compressed_size);
            let optional_file = result.unwrap();
            let mut file = optional_file.unwrap();
            let file_name = file.name().unwrap();
            assert_eq!(file_name, "name");
            // test reading
            let mut content = Vec::new();
            let read_result = file.read_to_end(&mut content);
            assert!(read_result.is_err()); // invalid checksum

            let error_str = read_result.unwrap_err().to_string();
            // eprintln!("{error_str}");
            assert!(error_str.contains("Invalid checksum"));
        }
        {
            // reading the file will work because we override the checksum
            let mut reader = Cursor::new(buffer.clone());
            let read_options = ZipReadOptions::default()
                .override_compressed_size(compressed_size)
                .override_crc(3632233996);
            let result = read_zipfile_from_stream_with_options(&mut reader, read_options);
            let optional_file = result.unwrap();
            let mut file = optional_file.unwrap();
            let file_name = file.name().unwrap();
            assert_eq!(file_name, "name");
            // test reading
            let mut content = Vec::new();
            file.read_to_end(&mut content).unwrap();
            assert_eq!(content, b"test");
        }
        {
            // reading the file will work because we skip checksum
            let mut reader = Cursor::new(buffer.clone());
            let read_options = ZipReadOptions::default()
                .override_compressed_size(compressed_size)
                .ignore_crc32(true);
            let result = read_zipfile_from_stream_with_options(&mut reader, read_options);
            let optional_file = result.unwrap();
            let mut file = optional_file.unwrap();
            let file_name = file.name().unwrap();
            assert_eq!(file_name, "name");
            // test reading
            let mut content = Vec::new();
            let read_result = file.read_to_end(&mut content);
            assert!(read_result.is_ok());
            assert_eq!(content, b"test");
        }
    }

    #[test]
    fn test_write_and_reopen_empty_archive() -> ZipResult<()> {
        use crate::read::read_zipfile_from_stream;
        use std::io::{Seek, SeekFrom};

        let writer = ZipWriter::new(Cursor::new(Vec::new()));
        let mut stream = writer.finish()?;
        stream.seek(SeekFrom::Start(0))?;
        assert!(read_zipfile_from_stream(&mut stream)?.is_none());
        Ok(())
    }

    #[test]
    fn test_write_and_reopen_empty_archive_with_comment() -> ZipResult<()> {
        use crate::read::read_zipfile_from_stream;
        use std::io::{Seek, SeekFrom};

        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.set_raw_comment(Box::new([255, 255]))?;
        let mut writer = ZipWriter::new_append(writer.finish()?)?;
        writer.set_raw_comment(Box::new([213, 255, 127, 255, 255, 255, 255, 0, 255, 7]))?;
        writer.set_raw_comment(Box::new([0]))?;
        let mut stream = writer.finish()?;
        stream.seek(SeekFrom::Start(0))?;
        assert!(read_zipfile_from_stream(&mut stream)?.is_none());
        Ok(())
    }
}
