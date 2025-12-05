//! Pipelined extraction into a filesystem directory.

#![cfg_attr(not(unix), allow(dead_code))]

pub mod path_splitting {
    use displaydoc::Display;
    use thiserror::Error;

    use std::collections::BTreeMap;

    use crate::spec::is_dir;

    /// Errors encountered during path splitting.
    #[derive(Debug, Display, Error)]
    pub enum PathSplitError<'a> {
        /// entry path {0:?} would escape extraction dir: {0:?}
        ExtractionPathEscapesDirectory(&'a str, &'static str),
        /// duplicate entry path {0:?} used: {0:?}
        DuplicatePath(&'a str, &'static str),
    }

    /* NB: path_to_string() performs some of this logic, but is intended to coerce filesystems
     * paths into zip entry names, whereas we are processing entry names from a zip archive, so we
     * perform much less error handling. */
    pub(crate) fn normalize_parent_dirs<'a>(
        entry_path: &'a str,
    ) -> Result<(Vec<&'a str>, bool), PathSplitError<'a>> {
        /* The ZIP spec states (APPNOTE 4.4.17) that file paths are in Unix format, and Unix
         * filesystems treat a backslash as a normal character. Thus they should be allowed on Unix
         * and replaced with \u{fffd} on Windows. */
        if entry_path.starts_with('/') {
            /* FIXME: we need to be able to accept absolute paths which resolve to somewhere under
             * the specified extraction directory (e.g. when an OS installer is unzipping the
             * contents of the root filesystem). These can be translated into unrooted paths. */
            return Err(PathSplitError::ExtractionPathEscapesDirectory(
                entry_path,
                "path began with '/' and is absolute",
            ));
        }
        let is_dir = is_dir(entry_path);

        let mut ret: Vec<&'a str> = Vec::new();
        for component in entry_path.split('/') {
            match component {
                /* Skip over repeated separators "//". We check separately for ending '/' with the
                 * `is_dir` variable. */
                "" => (),
                /* Skip over redundant "." separators. */
                "." => (),
                /* If ".." is present, pop off the last element or return an error. */
                ".." => {
                    if ret.pop().is_none() {
                        return Err(PathSplitError::ExtractionPathEscapesDirectory(
                            entry_path,
                            "path has too many '..' components and would escape the containing dir",
                        ));
                    }
                }
                _ => {
                    ret.push(component);
                }
            }
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
        /* This is a mutually recursive data structure (trie), so we need to box it up somewhere.
         * Boxing at this level allows b-tree values to be allocated within the b-tree allocation.
         * Note that this data structure only ever exists temporarily, and is consumed by
         * transform_entries_to_allocated_handles(). */
        #[allow(clippy::box_collection)]
        pub children: Box<BTreeMap<&'a str, FSEntry<'a, Data>>>,
    }

    impl<Data> Default for DirEntry<'_, Data> {
        fn default() -> Self {
            Self {
                properties: None,
                children: Box::new(BTreeMap::new()),
            }
        }
    }

    #[derive(PartialEq, Eq, Debug, Clone)]
    pub(crate) enum FSEntry<'a, Data> {
        Dir(DirEntry<'a, Data>),
        File(Data),
    }

    impl<'a, Data> From<DirEntry<'a, Data>> for FSEntry<'a, Data> {
        fn from(x: DirEntry<'a, Data>) -> Self {
            Self::Dir(x)
        }
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
    ) -> Result<BTreeMap<&'a str, FSEntry<'a, Data>>, PathSplitError<'a>>
    where
        Data: DirByMode,
    {
        let mut base_dir: DirEntry<'a, Data> = DirEntry::default();

        for (entry_path, data) in all_entries {
            /* Begin at the top-level directory. We will recurse downwards. */
            let mut cur_dir = &mut base_dir;

            /* Split entries by directory components, and normalize any non-literal paths
             * (e.g. '..', '.', leading '/', repeated '/'). */
            let (all_components, is_dir) = normalize_parent_dirs(entry_path)?;

            /* If the entry resolves to the top-level directory, we don't error, but instead just
             * avoid writing any data to that directory entry. */
            if all_components.is_empty() {
                continue;
            }

            /* If the entry is a directory by mode, then it does not need to end in '/'. */
            let is_dir = is_dir || data.is_dir_by_mode();
            /* Split basename and dirname. */
            let (dir_components, file_component) =
                split_dir_file_components(&all_components, is_dir);

            for component in dir_components.iter() {
                let next_subdir = cur_dir
                    .children
                    .entry(component)
                    .or_insert_with(|| DirEntry::default().into());
                cur_dir = match next_subdir {
                    FSEntry::File(_) => {
                        return Err(PathSplitError::DuplicatePath(
                            entry_path,
                            "a file was already registered at the same path as this dir entry",
                        ));
                    }
                    FSEntry::Dir(ref mut subdir) => subdir,
                }
            }
            match file_component {
                Some(filename) => {
                    /* We can't handle duplicate file paths, as that might mess up our
                     * parallelization strategy. */
                    if cur_dir.children.contains_key(filename) {
                        return Err(PathSplitError::DuplicatePath(
                            entry_path,
                            "an entry was already registered at the same path as this file entry",
                        ));
                    }
                    cur_dir.children.insert(filename, FSEntry::File(data));
                }
                None => {
                    /* We can't handle duplicate directory entries for the exact same normalized
                     * path, as it's not clear how to merge the possibility of two separate file
                     * permissions. */
                    if cur_dir.properties.replace(data).is_some() {
                        return Err(PathSplitError::DuplicatePath(
                            entry_path,
                            "another directory was already registered at this path",
                        ));
                    }
                }
            }
        }

        let DirEntry {
            properties,
            children,
        } = base_dir;
        debug_assert!(properties.is_none(), "setting metadata on the top-level extraction dir is not allowed and should have been filtered out");
        Ok(*children)
    }

    /* TODO: use proptest for all of this! */
    #[cfg(test)]
    mod test {
        use super::*;

        #[test]
        fn path_normalization() -> Result<(), PathSplitError<'static>> {
            assert_eq!(
                normalize_parent_dirs("a/b/c")?,
                (vec!["a", "b", "c"], false)
            );
            assert_eq!(normalize_parent_dirs("./a")?, (vec!["a"], false));
            assert_eq!(normalize_parent_dirs("a/../b/")?, (vec!["b"], true));
            assert_eq!(normalize_parent_dirs("a/")?, (vec!["a"], true));
            assert!(normalize_parent_dirs("/a").is_err());
            assert_eq!(normalize_parent_dirs("\\a")?, (vec!["\\a"], false));
            assert_eq!(normalize_parent_dirs("a\\b/")?, (vec!["a\\b"], true));
            assert!(normalize_parent_dirs("a/../../b").is_err());
            assert_eq!(normalize_parent_dirs("./")?, (vec![], true));
            Ok(())
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

        impl DirByMode for usize {
            fn is_dir_by_mode(&self) -> bool {
                false
            }
        }

        #[test]
        fn lex_trie() -> Result<(), PathSplitError<'static>> {
            assert_eq!(
                lexicographic_entry_trie([
                    ("a/b/", 1usize),
                    ("a/", 2),
                    ("a/b/c", 3),
                    ("d/", 4),
                    ("e", 5),
                    ("a/b/f/g", 6),
                ])?,
                [
                    (
                        "a",
                        FSEntry::Dir(DirEntry {
                            properties: Some(2),
                            children: Box::new(
                                [(
                                    "b",
                                    FSEntry::Dir(DirEntry {
                                        properties: Some(1),
                                        children: Box::new(
                                            [
                                                ("c", FSEntry::File(3)),
                                                (
                                                    "f",
                                                    FSEntry::Dir(DirEntry {
                                                        properties: None,
                                                        children: Box::new(
                                                            [("g", FSEntry::File(6))].into()
                                                        ),
                                                    })
                                                ),
                                            ]
                                            .into()
                                        ),
                                    })
                                )]
                                .into()
                            ),
                        })
                    ),
                    (
                        "d",
                        FSEntry::Dir(DirEntry {
                            properties: Some(4),
                            children: Box::new(BTreeMap::new()),
                        })
                    ),
                    ("e", FSEntry::File(5))
                ]
                .into()
            );
            Ok(())
        }

        #[test]
        fn lex_trie_dir_by_mode() -> Result<(), PathSplitError<'static>> {
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
                ])?,
                [
                    (
                        "a",
                        FSEntry::Dir(DirEntry {
                            properties: Some(Mode(2, false)),
                            children: Box::new(
                                [(
                                    "b",
                                    FSEntry::Dir(DirEntry {
                                        properties: Some(Mode(1, true)),
                                        children: Box::new(
                                            [
                                                ("c", FSEntry::File(Mode(3, false))),
                                                (
                                                    "f",
                                                    FSEntry::Dir(DirEntry {
                                                        properties: None,
                                                        children: Box::new(
                                                            [("g", FSEntry::File(Mode(6, false)))]
                                                                .into()
                                                        ),
                                                    })
                                                ),
                                            ]
                                            .into()
                                        ),
                                    })
                                )]
                                .into()
                            ),
                        })
                    ),
                    (
                        "d",
                        FSEntry::Dir(DirEntry {
                            properties: Some(Mode(4, true)),
                            children: Box::new(BTreeMap::new()),
                        })
                    ),
                    ("e", FSEntry::File(Mode(5, false)))
                ]
                .into()
            );
            Ok(())
        }
    }
}

#[cfg(unix)]
pub mod split_extraction {
    use by_address::ByAddress;
    use displaydoc::Display;
    use num_cpus;
    use thiserror::Error;

    use std::fs;
    use std::io;
    use std::mem;
    use std::path::Path;
    use std::pin::Pin;
    use std::sync::mpsc;
    use std::thread;

    use crate::compression::CompressionMethod;
    use crate::read::ZipArchive;
    use crate::read::{make_crypto_reader, make_reader};
    use crate::result::ZipError;
    use crate::spec::{FixedSizeBlock, Pod};
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

    use super::path_splitting::{lexicographic_entry_trie, PathSplitError};
    use crate::read::handle_creation::{transform_entries_to_allocated_handles, AllocatedHandles};

    /// Errors encountered during the split pipelined extraction process.
    #[derive(Debug, Display, Error)]
    pub enum SplitExtractionError {
        /// zip error: {0}
        Zip(#[from] ZipError),
        /// path split error: {0}
        PathSplit(String),
    }

    impl<'a> From<PathSplitError<'a>> for SplitExtractionError {
        fn from(e: PathSplitError<'a>) -> Self {
            let msg = format!("{}", e);
            Self::PathSplit(msg)
        }
    }

    impl From<io::Error> for SplitExtractionError {
        fn from(x: io::Error) -> Self {
            Self::Zip(x.into())
        }
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

        // Essentially the same logic as FixedSizeBlock::parse(), but adapted for pread().
        let block = {
            let mut block = ZipLocalEntryBlock::zeroed();

            input_file.pread_all(data.header_start, block.as_uninit_bytes_mut())?;

            // Convert endianness and check the magic value.
            ZipLocalEntryBlock::validate(block)?
        };

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
                debug_assert_eq!(*data.data_start.get().unwrap(), data_start);
            }
        }
        Ok(data_start)
    }

    /// Parameters to control the degree of parallelism used for extraction.
    #[derive(Debug, Clone)]
    pub struct ExtractionParameters {
        /// Number of threads used for decompression.
        ///
        /// Default value: number of available cpus via [`num_cpus::get()`].
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
                /* NB: this will perform a syscall. We still probably want to call this dynamically
                 * instead of globally caching the call, in order to respect the dynamic value of
                 * e.g. sched affinity. */
                decompression_threads: num_cpus::get() / 3,
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
        move || {
            if let Err(e) = f() {
                /* We use an async sender, so this should only error if the receiver has hung
                 * up, which occurs when we return a previous error from the main thread. */
                let _ = err_sender.send(e);
            }
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

        thread::scope(move |scope| {
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
                    #[allow(clippy::single_match)]
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

                                for (entry, mut output_file) in uncompressed_receiver.iter() {
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

                                for (entry, output_file) in compressed_receiver.iter() {
                                    /* Construct the decompressing reader. */
                                    let limited_reader =
                                        (&mut compressed_read_end).take(entry.compressed_size);
                                    let crypto_reader =
                                        make_crypto_reader(entry, limited_reader, None, None)?;
                                    let mut decompressing_reader = make_reader(
                                        entry.compression_method,
                                        entry.uncompressed_size,
                                        entry.crc32,
                                        crypto_reader,
                                        entry.flags,
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

                                for (entry, data_start, mut output_file) in read_recv.iter() {
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
                        #[allow(clippy::mutable_key_type)]
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
                        for entry in shared.files.values() {
                            /* We have already created all necessary directories, and we set any
                             * dir perms after extracting file contents. */
                            if entry.is_dir() || entry.is_dir_by_mode() {
                                continue;
                            }

                            /* Create a handle to the memory location of this entry. This allows
                             * us to quickly test membership without hashing any
                             * arbitrary-length strings/etc, and avoids the need to impl Hash/Eq
                             * on ZipFileData more generally. */
                            let handle = ByAddress(Pin::new(entry));
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
            if let Some(err) = err_receiver.iter().next() {
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

            #[cfg(feature = "_deflate-any")]
            let opts = opts.compression_method(CompressionMethod::Deflated);
            zip.start_file("d/e", opts.unix_permissions(0o755)).unwrap();
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
