//! Pipelined extraction into a filesystem directory.

pub mod path_splitting {
    use displaydoc::Display;
    use thiserror::Error;

    use std::collections::BTreeMap;

    use crate::spec::is_dir;

    /// Errors encountered during path splitting.
    #[derive(Debug, Display, Error)]
    pub enum PathSplitError {
        /// entry path format error: {0:?}
        PathFormat(String),
        /// file and directory paths overlapped: {0:?}
        FileDirOverlap(String),
    }

    fn split_by_separator<'a>(
        entry_path: &'a str,
    ) -> Result<impl Iterator<Item = &'a str>, PathSplitError> {
        if entry_path.contains('\\') {
            if entry_path.contains('/') {
                return Err(PathSplitError::PathFormat(format!(
                    "path {:?} contained both '\\' and '/' separators",
                    entry_path
                )));
            }
            Ok(entry_path.split('\\'))
        } else {
            Ok(entry_path.split('/'))
        }
    }

    /* TODO: consider using crate::unstable::path_to_string() for this--it involves new
     * allocations, but that really shouldn't matter for our purposes. I like the idea of using our
     * own logic here, since parallel/pipelined extraction is really a different use case than the
     * rest of the zip crate, but it's definitely worth considering. */
    pub(crate) fn normalize_parent_dirs<'a>(
        entry_path: &'a str,
    ) -> Result<(Vec<&'a str>, bool), PathSplitError> {
        if entry_path.starts_with('/') || entry_path.starts_with('\\') {
            return Err(PathSplitError::PathFormat(format!(
                "path {:?} began with '/' or '\\' and is absolute",
                entry_path
            )));
        }
        let is_dir = is_dir(entry_path);

        let mut ret: Vec<&'a str> = Vec::new();
        for component in split_by_separator(entry_path)? {
            match component {
                /* Skip over repeated separators "//". We check separately for ending '/' with the
                 * `is_dir` variable. */
                "" => (),
                /* Skip over redundant "." separators. */
                "." => (),
                /* If ".." is present, pop off the last element or return an error. */
                ".." => {
                    if ret.pop().is_none() {
                        return Err(PathSplitError::PathFormat(format!(
                        "path {:?} has too many '..' components and would escape the containing dir",
                        entry_path
                    )));
                    }
                }
                _ => {
                    ret.push(component);
                }
            }
        }
        if ret.is_empty() {
            return Err(PathSplitError::PathFormat(format!(
                "path {:?} resolves to the top-level directory",
                entry_path
            )));
        }

        Ok((ret, is_dir))
    }

    fn split_dir_file_components<'a, 's>(
        all_components: &'s [&'a str],
        is_dir: bool,
    ) -> (&'s [&'a str], Option<&'a str>) {
        if is_dir {
            (all_components, None)
        } else {
            let (last, rest) = all_components.split_last().unwrap();
            (rest, Some(last))
        }
    }

    #[derive(PartialEq, Eq, Debug, Clone)]
    pub(crate) struct DirEntry<'a, Data> {
        pub properties: Option<Data>,
        pub children: BTreeMap<&'a str, Box<FSEntry<'a, Data>>>,
    }

    impl<'a, Data> Default for DirEntry<'a, Data> {
        fn default() -> Self {
            Self {
                properties: None,
                children: BTreeMap::new(),
            }
        }
    }

    #[derive(PartialEq, Eq, Debug, Clone)]
    pub(crate) enum FSEntry<'a, Data> {
        Dir(DirEntry<'a, Data>),
        File(Data),
    }

    pub(crate) trait DirByMode {
        fn is_dir_by_mode(&self) -> bool;
    }

    impl DirByMode for &crate::types::ZipFileData {
        fn is_dir_by_mode(&self) -> bool {
            crate::types::ZipFileData::is_dir_by_mode(self)
        }
    }

    /* This returns a BTreeMap and not a DirEntry because we do not allow setting permissions or
     * any other data for the top-level extraction directory. */
    pub(crate) fn lexicographic_entry_trie<'a, Data>(
        all_entries: impl IntoIterator<Item = (&'a str, Data)>,
    ) -> Result<BTreeMap<&'a str, Box<FSEntry<'a, Data>>>, PathSplitError>
    where
        Data: DirByMode,
    {
        let mut base_dir: DirEntry<'a, Data> = DirEntry::default();

        for (entry_path, data) in all_entries {
            /* Begin at the top-level directory. We will recurse downwards. */
            let mut cur_dir = &mut base_dir;

            /* Split entries by directory components, and normalize any non-literal paths
             * (e.g. '..', '.', leading '/', repeated '/', similarly for windows '\\'). */
            let (all_components, is_dir) = normalize_parent_dirs(entry_path)?;
            /* If the entry is a directory by mode, then it does not need to end in '/'. */
            let is_dir = is_dir || data.is_dir_by_mode();
            /* Split basename and dirname. */
            let (dir_components, file_component) =
                split_dir_file_components(&all_components, is_dir);

            for component in dir_components.iter() {
                let next_subdir = cur_dir
                    .children
                    .entry(component)
                    .or_insert_with(|| Box::new(FSEntry::Dir(DirEntry::default())));
                cur_dir = match next_subdir.as_mut() {
                    &mut FSEntry::File(_) => {
                        return Err(PathSplitError::FileDirOverlap(format!(
                            "a file was already registered at the same path as the dir entry {:?}",
                            entry_path
                        )));
                    }
                    &mut FSEntry::Dir(ref mut subdir) => subdir,
                }
            }
            match file_component {
                Some(filename) => {
                    /* We can't handle duplicate file paths, as that might mess up our
                     * parallelization strategy. */
                    if let Some(_) = cur_dir.children.get(filename) {
                        return Err(PathSplitError::FileDirOverlap(format!(
                            "another file or directory was already registered at the same path as the file entry {:?}",
                            entry_path
                        )));
                    }
                    cur_dir
                        .children
                        .insert(filename, Box::new(FSEntry::File(data)));
                }
                None => {
                    /* We can't handle duplicate directory entries for the exact same normalized
                     * path, as it's not clear how to merge the possibility of two separate file
                     * permissions. */
                    if let Some(_) = cur_dir.properties.replace(data) {
                        return Err(PathSplitError::FileDirOverlap(format!(
                            "another directory was already registered at the path {:?}",
                            entry_path
                        )));
                    }
                }
            }
        }

        let DirEntry {
            properties,
            children,
        } = base_dir;
        assert!(properties.is_none(), "setting metadata on the top-level extraction dir is not allowed and should have been filtered out");
        Ok(children)
    }

    /* TODO: use proptest for all of this! */
    #[cfg(test)]
    mod test {
        use super::*;

        #[test]
        fn path_normalization() {
            assert_eq!(
                normalize_parent_dirs("a/b/c").unwrap(),
                (vec!["a", "b", "c"], false)
            );
            assert_eq!(normalize_parent_dirs("./a").unwrap(), (vec!["a"], false));
            assert_eq!(normalize_parent_dirs("a/../b/").unwrap(), (vec!["b"], true));
            assert_eq!(normalize_parent_dirs("a\\").unwrap(), (vec!["a"], true));
            assert!(normalize_parent_dirs("/a").is_err());
            assert!(normalize_parent_dirs("\\a").is_err());
            assert!(normalize_parent_dirs("a\\b/").is_err());
            assert!(normalize_parent_dirs("a/../../b").is_err());
            assert!(normalize_parent_dirs("./").is_err());
        }

        #[test]
        fn split_dir_file() {
            assert_eq!(
                split_dir_file_components(&["a", "b", "c"], true),
                (["a", "b", "c"].as_ref(), None)
            );
            assert_eq!(
                split_dir_file_components(&["a", "b", "c"], false),
                (["a", "b"].as_ref(), Some("c"))
            );
        }

        #[test]
        fn lex_trie() {
            impl DirByMode for usize {
                fn is_dir_by_mode(&self) -> bool {
                    false
                }
            }

            assert_eq!(
                lexicographic_entry_trie([
                    ("a/b/", 1usize),
                    ("a/", 2),
                    ("a/b/c", 3),
                    ("d/", 4),
                    ("e", 5),
                    ("a/b/f/g", 6),
                ])
                .unwrap(),
                [
                    (
                        "a",
                        FSEntry::Dir(DirEntry {
                            properties: Some(2),
                            children: [(
                                "b",
                                FSEntry::Dir(DirEntry {
                                    properties: Some(1),
                                    children: [
                                        ("c", FSEntry::File(3).into()),
                                        (
                                            "f",
                                            FSEntry::Dir(DirEntry {
                                                properties: None,
                                                children: [("g", FSEntry::File(6).into())]
                                                    .into_iter()
                                                    .collect(),
                                            })
                                            .into()
                                        ),
                                    ]
                                    .into_iter()
                                    .collect(),
                                })
                                .into()
                            )]
                            .into_iter()
                            .collect(),
                        })
                        .into()
                    ),
                    (
                        "d",
                        FSEntry::Dir(DirEntry {
                            properties: Some(4),
                            children: BTreeMap::new(),
                        })
                        .into()
                    ),
                    ("e", FSEntry::File(5).into())
                ]
                .into_iter()
                .collect()
            );
        }

        #[test]
        fn lex_trie_dir_by_mode() {
            #[derive(PartialEq, Eq, Debug)]
            struct Mode(usize, bool);

            impl DirByMode for Mode {
                fn is_dir_by_mode(&self) -> bool {
                    self.1
                }
            }

            assert_eq!(
                lexicographic_entry_trie([
                    ("a/b", Mode(1, true)),
                    ("a/", Mode(2, false)),
                    ("a/b/c", Mode(3, false)),
                    ("d", Mode(4, true)),
                    ("e", Mode(5, false)),
                    ("a/b/f/g", Mode(6, false)),
                ])
                .unwrap(),
                [
                    (
                        "a",
                        FSEntry::Dir(DirEntry {
                            properties: Some(Mode(2, false)),
                            children: [(
                                "b",
                                FSEntry::Dir(DirEntry {
                                    properties: Some(Mode(1, true)),
                                    children: [
                                        ("c", FSEntry::File(Mode(3, false)).into()),
                                        (
                                            "f",
                                            FSEntry::Dir(DirEntry {
                                                properties: None,
                                                children: [(
                                                    "g",
                                                    FSEntry::File(Mode(6, false)).into()
                                                )]
                                                .into_iter()
                                                .collect(),
                                            })
                                            .into()
                                        ),
                                    ]
                                    .into_iter()
                                    .collect(),
                                })
                                .into()
                            )]
                            .into_iter()
                            .collect(),
                        })
                        .into()
                    ),
                    (
                        "d",
                        FSEntry::Dir(DirEntry {
                            properties: Some(Mode(4, true)),
                            children: BTreeMap::new(),
                        })
                        .into()
                    ),
                    ("e", FSEntry::File(Mode(5, false)).into())
                ]
                .into_iter()
                .collect()
            );
        }
    }
}

pub mod handle_creation {
    use displaydoc::Display;
    use thiserror::Error;

    use std::cmp;
    use std::collections::{HashMap, VecDeque};
    use std::fs;
    use std::hash;
    use std::io;
    use std::path::{Path, PathBuf};

    use crate::types::ZipFileData;

    use super::path_splitting::{DirEntry, FSEntry};

    /// Errors encountered when creating output handles for extracting entries to.
    #[derive(Debug, Display, Error)]
    pub enum HandleCreationError {
        /// i/o error: {0}
        Io(#[from] io::Error),
    }

    /// Wrapper for memory location of the ZipFileData in Shared.
    ///
    /// Enables quick comparison and hash table lookup without needing to implement Hash/Eq for
    /// ZipFileData more generally.
    #[derive(Debug)]
    pub(crate) struct ZipDataHandle<'a>(&'a ZipFileData);

    impl<'a> ZipDataHandle<'a> {
        #[inline(always)]
        const fn ptr(&self) -> *const ZipFileData {
            self.0
        }

        #[inline(always)]
        pub const fn wrap(data: &'a ZipFileData) -> Self {
            Self(data)
        }
    }

    impl<'a> cmp::PartialEq for ZipDataHandle<'a> {
        #[inline(always)]
        fn eq(&self, other: &Self) -> bool {
            self.ptr() == other.ptr()
        }
    }

    impl<'a> cmp::Eq for ZipDataHandle<'a> {}

    impl<'a> hash::Hash for ZipDataHandle<'a> {
        #[inline(always)]
        fn hash<H: hash::Hasher>(&self, state: &mut H) {
            self.ptr().hash(state);
        }
    }

    /* TODO: figure out how to handle symlinks! These are especially difficult because:
     * (1) windows symlinks files and directories differently, and only on newer windows versions,
     * (2) later entries in the zip may refer to symlink paths from earlier in the zip.
     *
     * Of these issues, (2) is more difficult and intrinsic to the problem space. In order to
     * correctly extract symlinks in a pipelined/parallel fashion, we need to identify these order
     * dependencies and schedule the symlink dereference (reading the target value from the zip)
     * before we create any directories or allocate any output file handles that dereference that
     * symlink. This is less of a problem with the synchronous in-order extraction because it
     * creates any symlinks immediately (it imposes a total ordering dependency over all entries).
     */
    pub(crate) struct AllocatedHandles<'a> {
        pub file_handle_mapping: HashMap<ZipDataHandle<'a>, fs::File>,
        pub perms_todo: Vec<(PathBuf, fs::Permissions)>,
    }

    pub(crate) fn transform_entries_to_allocated_handles<'a>(
        top_level_extraction_dir: &Path,
        lex_entry_trie: impl IntoIterator<Item = (&'a str, Box<FSEntry<'a, &'a ZipFileData>>)>,
    ) -> Result<AllocatedHandles<'a>, HandleCreationError> {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        /* TODO: we create subdirs by constructing path strings, which may fail at overlarge
         * paths. This may be fixable on unix with mkdirat()/openat(), but would require more
         * complex platform-specific programming. However, the result would likely decrease the
         * number of syscalls, which may also improve performance. It may also be slightly easier to
         * follow the logic if we can refer to directory inodes instead of constructing path strings
         * as a proxy. This should be considered if requested by users. */
        fs::create_dir_all(top_level_extraction_dir)?;

        let mut file_handle_mapping: HashMap<ZipDataHandle<'a>, fs::File> = HashMap::new();
        let mut entry_queue: VecDeque<(PathBuf, Box<FSEntry<'a, &'a ZipFileData>>)> =
            lex_entry_trie
                .into_iter()
                .map(|(entry_name, entry_data)| {
                    (top_level_extraction_dir.join(entry_name), entry_data)
                })
                .collect();
        let mut perms_todo: Vec<(PathBuf, fs::Permissions)> = Vec::new();

        while let Some((path, entry)) = entry_queue.pop_front() {
            match *entry {
                FSEntry::File(data) => {
                    let key = ZipDataHandle::wrap(data);

                    if let Some(mode) = data.unix_mode() {
                        /* TODO: consider handling the readonly bit on windows. We don't currently
                         * do this in normal extraction, so we don't need to do this yet for
                         * pipelining. */

                        /* Write the desired perms to the perms queue. */
                        #[cfg(unix)]
                        perms_todo.push((path.clone(), fs::Permissions::from_mode(mode)));
                    }

                    let handle = fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .open(path)?;
                    assert!(file_handle_mapping.insert(key, handle).is_none());
                }
                FSEntry::Dir(DirEntry {
                    properties,
                    children,
                }) => {
                    match fs::create_dir(&path) {
                        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                        Err(e) => return Err(e.into()),
                        Ok(()) => (),
                    }

                    /* (1) Write any desired perms to the perms queue. */
                    #[cfg(unix)]
                    if let Some(perms_to_set) = properties.and_then(|data| data.unix_mode()) {
                        perms_todo.push((path.clone(), fs::Permissions::from_mode(perms_to_set)));
                    }
                    /* (2) Generate sub-entries by constructing full paths. */
                    for (sub_name, entry) in children.into_iter() {
                        let full_name = path.join(sub_name);
                        entry_queue.push_back((full_name, entry));
                    }
                }
            }
        }

        /* NB: Iterate in *REVERSE* so that child directories are set before parents! Setting
         * a parent readonly would stop us from setting child perms. */
        perms_todo.reverse();

        Ok(AllocatedHandles {
            file_handle_mapping,
            perms_todo,
        })
    }

    #[cfg(test)]
    mod test {
        use tempdir::TempDir;

        use std::io::{prelude::*, Cursor};

        use crate::write::{SimpleFileOptions, ZipWriter};

        use super::super::path_splitting::lexicographic_entry_trie;
        use super::*;

        #[test]
        fn subdir_creation() {
            #[cfg(unix)]
            use std::os::unix::fs::PermissionsExt;

            /* Create test archive. */
            let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
            let opts = SimpleFileOptions::default();

            zip.start_file("a/b/c", opts.unix_permissions(0o765))
                .unwrap();
            zip.write_all(b"asdf").unwrap();

            zip.add_directory("a/b", opts.unix_permissions(0o500))
                .unwrap();

            /* Create readable archive and extraction dir. */
            let zip = zip.finish_into_readable().unwrap();
            let td = TempDir::new("pipeline-test").unwrap();

            /* (1) Create lex entry trie. */
            let trie = lexicographic_entry_trie(
                zip.shared
                    .files
                    .iter()
                    .map(|(name, data)| (name.as_ref(), data)),
            )
            .unwrap();

            /* (2) Generate handles. */
            let AllocatedHandles {
                file_handle_mapping,
                perms_todo,
            } = transform_entries_to_allocated_handles(td.path(), trie).unwrap();

            let mut files: Vec<_> = file_handle_mapping.into_iter().collect();
            assert_eq!(1, files.len());
            let (file_data, mut file) = files.pop().unwrap();
            assert_eq!(
                file_data,
                ZipDataHandle::wrap(zip.shared.files.get_index(0).unwrap().1)
            );
            /* We didn't write anything to the file, so it's still empty at this point. */
            assert_eq!(b"", &fs::read(td.path().join("a/b/c")).unwrap()[..]);
            file.write_all(b"asdf").unwrap();
            file.sync_data().unwrap();
            /* Now the data is synced! */
            assert_eq!(b"asdf", &fs::read(td.path().join("a/b/c")).unwrap()[..]);

            #[cfg(unix)]
            assert_eq!(
                perms_todo
                    .into_iter()
                    .map(|(path, perms)| (path, perms.mode() & 0o777))
                    .collect::<Vec<_>>(),
                vec![
                    (td.path().join("a/b/c"), 0o765),
                    (td.path().join("a/b"), 0o500),
                ]
            );
        }
    }
}

#[cfg(unix)]
pub mod split_extraction {
    use displaydoc::Display;
    use thiserror::Error;

    use std::fs;
    use std::io;
    use std::mem::{self, MaybeUninit};
    use std::path::Path;
    use std::sync::mpsc;
    use std::thread;

    use crate::compression::CompressionMethod;
    use crate::read::ZipArchive;
    use crate::read::{make_crypto_reader, make_reader};
    use crate::result::ZipError;
    use crate::spec::FixedSizeBlock;
    use crate::types::{ZipFileData, ZipLocalEntryBlock};

    #[cfg(not(target_os = "linux"))]
    use crate::read::split::pipe::unix::{PipeReadBufferSplicer, PipeWriteBufferSplicer};
    #[cfg(target_os = "linux")]
    use crate::read::split::{
        file::linux::FileCopy,
        pipe::linux::{PipeReadSplicer, PipeWriteSplicer},
    };
    use crate::read::split::{
        file::{
            unix::{FileBufferCopy, FileInput, FileOutput},
            CopyRange, InputFile,
        },
        pipe::{unix::create_pipe, ReadSplicer, WriteSplicer},
        util::{copy_via_buf, TakeWrite},
    };

    use super::{
        handle_creation::{
            transform_entries_to_allocated_handles, AllocatedHandles, HandleCreationError,
            ZipDataHandle,
        },
        path_splitting::{lexicographic_entry_trie, PathSplitError},
    };

    /// Errors encountered during the split pipelined extraction process.
    #[derive(Debug, Display, Error)]
    pub enum SplitExtractionError {
        /// i/o error: {0}
        Io(#[from] io::Error),
        /// zip error: {0}
        Zip(#[from] ZipError),
        /// path split error: {0}
        PathSplit(#[from] PathSplitError),
        /// handle creation error: {0}
        HandleCreation(#[from] HandleCreationError),
    }

    /* TODO: make this share code with find_data_start()! */
    fn get_or_find_data_start<InF>(data: &ZipFileData, input_file: InF) -> Result<u64, ZipError>
    where
        InF: InputFile,
    {
        // TODO: use .get_or_try_init() once stabilized to provide a closure returning a Result!
        if let Some(data_start) = data.data_start.get() {
            return Ok(*data_start);
        }

        let block = {
            let block: MaybeUninit<[u8; mem::size_of::<ZipLocalEntryBlock>()]> =
                MaybeUninit::uninit();
            let mut block: [MaybeUninit<u8>; mem::size_of::<ZipLocalEntryBlock>()] =
                unsafe { mem::transmute(block) };

            input_file.pread_all(data.header_start, &mut block[..])?;

            let block: MaybeUninit<[u8; mem::size_of::<ZipLocalEntryBlock>()]> =
                unsafe { mem::transmute(block) };
            unsafe { block.assume_init() }
        };
        // Parse static-sized fields and check the magic value.
        let block = ZipLocalEntryBlock::interpret(block.as_ref())?;

        // Calculate the end of the local header from the fields we just parsed.
        let variable_fields_len: u64 =
            // Each of these fields must be converted to u64 before adding, as the result may
            // easily overflow a u16.
            block.file_name_length as u64 + block.extra_field_length as u64;
        let local_entry_block_size: u64 = mem::size_of::<ZipLocalEntryBlock>().try_into().unwrap();
        let data_start: u64 = data.header_start + local_entry_block_size + variable_fields_len;

        // Set the value so we don't have to read it again.
        match data.data_start.set(data_start) {
            Ok(()) => (),
            // If the value was already set in the meantime, ensure it matches.
            Err(_) => {
                assert_eq!(*data.data_start.get().unwrap(), data_start);
            }
        }
        Ok(data_start)
    }

    /// Parameters to control the degree of parallelism used for extraction.
    #[derive(Debug, Clone)]
    pub struct ExtractionParameters {
        /// Number of threads used for decompression.
        ///
        /// Default value: 4.
        ///
        /// Note that multiple times this many threads will be spawned by [`split_extract()`] as
        /// part of the pipelined process. Only this many threads will be used to perform
        /// decompression in rust code, but other threads will be used to wait on I/O from
        /// the kernel.
        pub decompression_threads: usize,
        /// Size of buffer used to copy a decompressed entry into the corresponding output pipe.
        ///
        /// Default value: 1MB.
        pub decompression_copy_buffer_length: usize,
        /// Size of buffer used to copy stored entries into the output file.
        ///
        /// Used on non-Linux platforms without
        /// [`copy_file_range()`](https://www.gnu.org/software/libc/manual/html_node/Copying-File-Data.html),
        /// as well as on Linux when the input and output file handles are on separate devices.
        ///
        /// Default value: 1MB.
        pub file_range_copy_buffer_length: usize,
        /// Size of buffer used to splice contents from a pipe into an output file handle.
        ///
        /// Used on non-Linux platforms without [`splice()`](https://en.wikipedia.org/wiki/Splice_(system_call)).
        ///
        /// Default value: 1MB.
        #[cfg(not(target_os = "linux"))]
        pub splice_read_buffer_length: usize,
        /// Size of buffer used to splice contents from an input file handle into a pipe.
        ///
        /// Used on non-Linux platforms without [`splice()`](https://en.wikipedia.org/wiki/Splice_(system_call)).
        ///
        /// Default value: 1MB.
        #[cfg(not(target_os = "linux"))]
        pub splice_write_buffer_length: usize,
    }

    impl Default for ExtractionParameters {
        fn default() -> Self {
            Self {
                decompression_threads: 4,
                decompression_copy_buffer_length: 1024 * 1024,
                file_range_copy_buffer_length: 1024 * 1024,
                #[cfg(not(target_os = "linux"))]
                splice_read_buffer_length: 1024 * 1024,
                #[cfg(not(target_os = "linux"))]
                splice_write_buffer_length: 1024 * 1024,
            }
        }
    }

    fn wrap_spawn_err<'scope>(
        err_sender: mpsc::Sender<SplitExtractionError>,
        f: impl FnOnce() -> Result<(), SplitExtractionError> + Send + 'scope,
    ) -> impl FnOnce() + Send + 'scope {
        move || match f() {
            Ok(()) => (),
            Err(e) => match err_sender.send(e) {
                Ok(()) => (),
                /* We use an async sender, so this should only error if the receiver has hung
                 * up, which occurs when we return a previous error from the main thread. */
                Err(mpsc::SendError(_)) => (),
            },
        }
    }

    /// Extract all entries in parallel using a pipelined strategy.
    pub fn split_extract(
        archive: &ZipArchive<fs::File>,
        top_level_extraction_dir: &Path,
        params: ExtractionParameters,
    ) -> Result<(), SplitExtractionError> {
        let ZipArchive {
            reader: ref input_file,
            ref shared,
            ..
        } = archive;
        let ExtractionParameters {
            decompression_threads,
            decompression_copy_buffer_length,
            file_range_copy_buffer_length,
            #[cfg(not(target_os = "linux"))]
            splice_read_buffer_length,
            #[cfg(not(target_os = "linux"))]
            splice_write_buffer_length,
        } = params;

        /* (1) Create lex entry trie. */
        let trie = lexicographic_entry_trie(
            shared
                .files
                .iter()
                .map(|(name, data)| (name.as_ref(), data)),
        )?;
        /* (2) Generate handles. */
        let AllocatedHandles {
            file_handle_mapping,
            perms_todo,
        } = transform_entries_to_allocated_handles(top_level_extraction_dir, trie)?;

        /* (3) Create a wrapper over the input file which uses pread() to read from multiple
         *     sections in parallel across a thread pool. */
        let input_file = FileInput::new(input_file)?;

        thread::scope(move |ref scope| {
            /* (4) Create n parallel consumer pipelines. Threads are spawned into the scope, so
             *     panics get propagated automatically, and all threads are joined at the end of the
             *     scope. wrap_spawn_err() is used to enable thread closures to return a Result and
             *     asynchronously propagate the error back up to the main scope thread. */
            let (err_sender, err_receiver) = mpsc::channel::<SplitExtractionError>();
            /* This channel is used to notify the zip-input-reader thread when a consumer has
             * completed decompressing/copying an entry, and is ready to receive new input. This is
             * neither round-robin nor LRU: no thread is prioritized over any other, and new
             * entries are sent off to workers in order of when they notify the zip-input-reader
             * thread of their readiness. */
            let (queue_sender, queue_receiver) = mpsc::channel::<usize>();
            let input_writer_infos: Vec<mpsc::Sender<(&ZipFileData, u64, FileOutput)>> = (0
                ..decompression_threads)
                .map(|consumer_index| {
                    /* Create pipes to write entries through. */
                    let (compressed_read_end, compressed_write_end) = create_pipe()?;
                    let (uncompressed_read_end, uncompressed_write_end) = create_pipe()?;

                    /* Create channels to send entries through. */
                    let (read_send, read_recv) = mpsc::channel::<(&ZipFileData, u64, FileOutput)>();
                    let (compressed_sender, compressed_receiver) =
                        mpsc::channel::<(&ZipFileData, FileOutput)>();
                    let (uncompressed_sender, uncompressed_receiver) =
                        mpsc::channel::<(&ZipFileData, FileOutput)>();

                    /* Send this consumer pipeline's index to the zip-input-reader thread when it's
                     * ready to receive new input. */
                    let queue_sender = queue_sender.clone();
                    let notify_readiness = move || match queue_sender.send(consumer_index) {
                        Ok(()) => (),
                        /* Disconnected; this is expected to occur at the end of extraction. */
                        Err(mpsc::SendError(_)) => (),
                    };

                    /* (8) Write decompressed entries to the preallocated output handles. */
                    thread::Builder::new()
                        .name(format!("zip-output-writer-{}", consumer_index))
                        .spawn_scoped(
                            scope,
                            wrap_spawn_err(err_sender.clone(), move || {
                                let uncompressed_receiver = uncompressed_receiver;
                                let mut uncompressed_read_end = uncompressed_read_end;

                                #[cfg(target_os = "linux")]
                                let mut s = PipeReadSplicer;
                                #[cfg(not(target_os = "linux"))]
                                let mut splice_buf: Box<[u8]> =
                                    vec![0u8; splice_read_buffer_length].into_boxed_slice();
                                #[cfg(not(target_os = "linux"))]
                                let mut s = PipeReadBufferSplicer::new(&mut splice_buf);

                                for (ref entry, mut output_file) in uncompressed_receiver.iter() {
                                    s.splice_to_file_all(
                                        &mut uncompressed_read_end,
                                        (&mut output_file, 0),
                                        entry.uncompressed_size.try_into().unwrap(),
                                    )?;
                                    let output_file = output_file.into_file();
                                    output_file.sync_data()?;
                                    mem::drop(output_file);
                                }

                                Ok(())
                            }),
                        )?;

                    /* (7) Read compressed entries, decompress them, then write them to the output
                     *     thread. */
                    thread::Builder::new()
                        .name(format!("zip-decompressor-{}", consumer_index))
                        .spawn_scoped(
                            scope,
                            wrap_spawn_err(err_sender.clone(), move || {
                                use io::{Read, Write};

                                let compressed_receiver = compressed_receiver;
                                let uncompressed_sender = uncompressed_sender;
                                let mut compressed_read_end = compressed_read_end;
                                let mut uncompressed_write_end = uncompressed_write_end;

                                /* Create a persistent heap-allocated buffer to copy decompressed
                                 * data through. We will be reusing this allocation, so pay the cost
                                 * of initialization exactly once. */
                                let mut buffer_allocation: Box<[u8]> =
                                    vec![0u8; decompression_copy_buffer_length].into_boxed_slice();

                                for (ref entry, output_file) in compressed_receiver.iter() {
                                    /* Construct the decompressing reader. */
                                    let limited_reader = ((&mut compressed_read_end)
                                        as &mut dyn Read)
                                        .take(entry.compressed_size);
                                    let crypto_reader = make_crypto_reader(
                                        entry.compression_method,
                                        entry.crc32,
                                        None,
                                        false,
                                        limited_reader,
                                        None,
                                        None,
                                        #[cfg(feature = "aes-crypto")]
                                        entry.compressed_size,
                                    )?;
                                    let mut decompressing_reader = make_reader(
                                        entry.compression_method,
                                        entry.crc32,
                                        crypto_reader,
                                    )?;
                                    let mut limited_writer = TakeWrite::take(
                                        uncompressed_write_end.by_ref(),
                                        entry.uncompressed_size,
                                    );
                                    /* Send the entry and output file to the writer thread before
                                     * writing this entry's decompressed contents. */
                                    uncompressed_sender.send((entry, output_file)).unwrap();
                                    copy_via_buf(
                                        &mut decompressing_reader,
                                        &mut limited_writer,
                                        &mut buffer_allocation,
                                    )?;
                                }

                                Ok(())
                            }),
                        )?;

                    /* (6) Wait on splicing the data from this entry, or using copy_file_range() to
                     *     copy it if uncompressed. */
                    thread::Builder::new()
                        .name(format!("zip-reader-{}", consumer_index))
                        .spawn_scoped(
                            scope,
                            wrap_spawn_err(err_sender.clone(), move || {
                                let notify_readiness = notify_readiness;
                                let read_recv = read_recv;
                                let compressed_sender = compressed_sender;
                                let mut compressed_write_end = compressed_write_end;

                                let mut copy_buf: Box<[u8]> =
                                    vec![0u8; file_range_copy_buffer_length].into_boxed_slice();
                                let mut buffer_c = FileBufferCopy::new(&mut copy_buf);

                                #[cfg(target_os = "linux")]
                                let mut s = PipeWriteSplicer::new();
                                #[cfg(not(target_os = "linux"))]
                                let mut splice_buf: Box<[u8]> =
                                    vec![0u8; splice_write_buffer_length].into_boxed_slice();
                                #[cfg(not(target_os = "linux"))]
                                let mut s = PipeWriteBufferSplicer::new(&mut splice_buf);

                                /* Notify readiness *after* setting up copy buffers, but *before*
                                 * waiting on any entries sent from the zip-input-reader thread,
                                 * since zip-input-reader won't send us anything over `read_recv`
                                 * until we notify them. */
                                notify_readiness();

                                for (ref entry, data_start, mut output_file) in read_recv.iter() {
                                    /* If uncompressed, we can use copy_file_range() directly, and
                                     * avoid splicing through our decompression pipeline. */
                                    if entry.compression_method == CompressionMethod::Stored {
                                        assert_eq!(entry.compressed_size, entry.uncompressed_size);
                                        let copy_len: usize =
                                            entry.uncompressed_size.try_into().unwrap();

                                        #[cfg(target_os = "linux")]
                                        if input_file.on_same_device(&output_file)? {
                                            /* Linux can map pages from one file to another
                                             * directly, without copying through userspace, but
                                             * only if the files are located on the same device. */
                                            let mut file_c = FileCopy::new();
                                            file_c.copy_file_range_all(
                                                (&input_file, data_start),
                                                (&mut output_file, 0),
                                                copy_len,
                                            )?;
                                        } else {
                                            buffer_c.copy_file_range_all(
                                                (&input_file, data_start),
                                                (&mut output_file, 0),
                                                copy_len,
                                            )?;
                                        }
                                        #[cfg(not(target_os = "linux"))]
                                        buffer_c.copy_file_range_all(
                                            (&input_file, data_start),
                                            (&mut output_file, 0),
                                            copy_len,
                                        )?;

                                        let output_file = output_file.into_file();
                                        /* fsync(2) says setting the file length is a form of
                                         * metadata that requires fsync() over fdatasync(); it's
                                         * unclear whether rust already performs that in the
                                         * File::set_len() call performed in the OutputFile::new()
                                         * constructor, but this shouldn't really matter for
                                         * performance. */
                                        output_file.sync_all()?;
                                        /* This is done automatically, but this way we can ensure
                                         * we've correctly avoided aliasing the output file in this
                                         * branch. */
                                        mem::drop(output_file);

                                        /* We're now completely done with this entry and have
                                         * closed the output file handle, so we can receive another
                                         * one. */
                                        notify_readiness();
                                        continue;
                                    }

                                    /* If compressed, we want to perform decompression (rust-level
                                     * synchronous computation) in a separate thread, to avoid
                                     * jumping back and forth in the call stack between i/o from the
                                     * kernel and in-memory computation in rust. */
                                    compressed_sender.send((entry, output_file)).unwrap();

                                    /* Write this uncompressed entry into the waiting pipe. Because
                                     * unix pipes have a constant non-configurable buffer size of
                                     * PIPE_BUF (on Linux, this is 4096 bytes; see pipe(7)), we will
                                     * end up blocking on this repeated splice() call until almost
                                     * the entire entry is decompressed in the decompressor thread.
                                     * This is a nice form of built-in flow control for other use
                                     * cases, but for our purposes we might like to use larger
                                     * buffers so that we can get further ahead in I/O from the
                                     * zip-input-reader thread. However, if we avoid using pipes
                                     * from the kernel, we won't be able to take advantage of
                                     * splice()'s zero-copy optimization on Linux. */
                                    /* TODO: consider using rust-level ring buffers here with
                                     * configurable size on all platforms, trading greater memory
                                     * allocation for further I/O readahead throughput. */
                                    s.splice_from_file_all(
                                        (&input_file, data_start),
                                        &mut compressed_write_end,
                                        entry.compressed_size.try_into().unwrap(),
                                    )?;

                                    /* Notify the zip-input-reader thread that we are ready to
                                     * read another entry's bytes from the input file handle. */
                                    notify_readiness();
                                }

                                Ok(())
                            }),
                        )?;

                    Ok(read_send)
                })
                .collect::<Result<_, SplitExtractionError>>()?;

            /* (5) Iterate over each entry sequentially, farming it out to a pipe to decompress if
             *     needed. */
            thread::Builder::new()
                .name("zip-input-reader".to_string())
                .spawn_scoped(
                    scope,
                    wrap_spawn_err(err_sender, move || {
                        let mut file_handle_mapping = file_handle_mapping;
                        /* All consumer pipelines share the same channel to notify us of their
                         * identity when ready. */
                        let queue_receiver = queue_receiver;
                        /* The only output channel we have to consumer pipelines is a single sender
                         * to notify them of the current entry they should be reading,
                         * decompressing, then writing to the preallocated output file handle. */
                        let read_sends: Vec<mpsc::Sender<(&ZipFileData, u64, FileOutput)>> =
                            input_writer_infos;

                        /* Entries are ordered by their offset, so we will be going monotonically
                         * forward in the underlying file. */
                        for ref entry in shared.files.values() {
                            /* We have already created all necessary directories, and we set any
                             * dir perms after extracting file contents. */
                            if entry.is_dir() || entry.is_dir_by_mode() {
                                continue;
                            }

                            /* Create a handle to the memory location of this entry. This allows
                             * us to quickly test membership without hashing any
                             * arbitrary-length strings/etc, and avoids the need to impl Hash/Eq
                             * on ZipFileData more generally. */
                            let handle = ZipDataHandle::wrap(entry);
                            /* Wrap the preallocated output handle for this entry in our
                             * linux-specific wrapper. */
                            let output_file = file_handle_mapping.remove(&handle).unwrap();
                            /* Set the length of the output handle according to the known output
                             * size. */
                            let output_file =
                                FileOutput::new(output_file, entry.uncompressed_size)?;

                            /* Get the start of data for this entry without mutating any state
                             * using pread. */
                            let data_start = get_or_find_data_start(entry, input_file)?;

                            /* Wait until a free consumer is available, then send the prepared
                             * entry range into the waiting consumer thread. */
                            let ready_consumer_index = queue_receiver.recv().unwrap();
                            read_sends[ready_consumer_index]
                                .send((entry, data_start, output_file))
                                .unwrap();
                        }

                        assert!(file_handle_mapping.is_empty());

                        Ok(())
                    }),
                )?;

            /* If no I/O errors occurred, this won't trigger. We will only be able to propagate
             * a single I/O error, but this also avoids propagating any errors triggered after the
             * initial one. */
            for err in err_receiver.iter() {
                return Err(err);
            }

            /* (10) Set permissions on specified entries. */
            /* TODO: consider parallelizing this with rayon's parallel iterators. */
            for (entry_path, perms) in perms_todo.into_iter() {
                fs::set_permissions(entry_path, perms)?;
            }

            Ok(())
        })
    }

    #[cfg(test)]
    mod test {
        use tempdir::TempDir;
        use tempfile;

        use std::io::prelude::*;

        use crate::write::{SimpleFileOptions, ZipWriter};

        use super::*;

        #[test]
        fn subdir_creation() {
            #[cfg(unix)]
            use std::os::unix::fs::PermissionsExt;

            /* Create test archive. */
            let mut zip = ZipWriter::new(tempfile::tempfile().unwrap());
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

            zip.start_file("a/b/c", opts.unix_permissions(0o765))
                .unwrap();
            zip.write_all(b"asdf").unwrap();

            zip.add_directory("a/b", opts.unix_permissions(0o500))
                .unwrap();

            zip.start_file(
                "d/e",
                opts.compression_method(CompressionMethod::Deflated)
                    .unix_permissions(0o755),
            )
            .unwrap();
            zip.write_all(b"ffasedfasjkef").unwrap();

            /* Create readable archive and extraction dir. */
            let zip = zip.finish_into_readable().unwrap();
            let td = TempDir::new("pipeline-test").unwrap();

            /* Perform the whole end-to-end extraction process. */
            split_extract(&zip, td.path(), ExtractionParameters::default()).unwrap();

            #[cfg(unix)]
            assert_eq!(
                0o765,
                fs::metadata(td.path().join("a/b/c"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777
            );
            assert_eq!(b"asdf", &fs::read(td.path().join("a/b/c")).unwrap()[..]);

            #[cfg(unix)]
            assert_eq!(
                0o500,
                fs::metadata(td.path().join("a/b"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
            );

            #[cfg(unix)]
            assert_eq!(
                0o755,
                fs::metadata(td.path().join("d/e"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777
            );
            assert_eq!(
                b"ffasedfasjkef",
                &fs::read(td.path().join("d/e")).unwrap()[..]
            );
        }
    }
}
