#![allow(unexpected_cfgs)] // Needed for cfg(fuzzing) on nightly as of 2024-05-06

use arbitrary::{Arbitrary, Unstructured};
use replace_with::replace_with_or_abort;
use std::fmt::Write;
use std::io::{Cursor, Seek, SeekFrom};
use std::io::{Write as IoWrite, copy, sink};
use std::ops;
use std::path::PathBuf;
#[cfg(fuzzing)]
use tikv_jemallocator::Jemalloc;
use zip::read::read_zipfile_from_stream;
use zip::result::{ZipError, ZipResult};
use zip::unstable::path_to_string;
use zip::write::FullFileOptions;
use zip::{ZipArchive, ZipWriter};

#[cfg(fuzzing)]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

#[derive(Arbitrary, Clone, Debug)]
pub enum BasicFileOperation<'k, 'n> {
    WriteNormalFile {
        contents: Box<[Box<[u8]>]>,
        options: FullFileOptions<'k, 'n>,
    },
    WriteDirectory(FullFileOptions<'k, 'n>),
    WriteSymlinkWithTarget {
        target: PathBuf,
        options: FullFileOptions<'k, 'n>,
    },
    ShallowCopy(Box<FileOperation<'k, 'n>>),
    DeepCopy(Box<FileOperation<'k, 'n>>),
    MergeWithOtherFile {
        initial_junk: Box<[u8]>,
        operations: Box<[(FileOperation<'k, 'n>, bool)]>,
    },
    SetArchiveComment(Box<[u8]>),
}

#[derive(Arbitrary, Copy, Clone, Debug, Eq, PartialEq)]
pub enum ReopenOption {
    DoNotReopen,
    ViaFinish,
    ViaFinishIntoReadable,
}

#[derive(Arbitrary, Clone, Debug)]
pub struct FileOperation<'k, 'n> {
    basic: BasicFileOperation<'k, 'n>,
    path: PathBuf,
    reopen: ReopenOption,
    // 'abort' flag is separate, to prevent trying to copy an aborted file
}

impl FileOperation<'_, '_> {
    fn get_path(&self) -> Option<PathBuf> {
        let mut pending = vec![self];
        while let Some(operation) = pending.pop() {
            match &operation.basic {
                BasicFileOperation::SetArchiveComment(_) => {}
                BasicFileOperation::WriteDirectory(_) => {
                    return Some(operation.path.join(""));
                }
                BasicFileOperation::MergeWithOtherFile { operations, .. } => {
                    pending.extend(
                        operations
                            .iter()
                            .rev()
                            .filter_map(|(operation, abort)| (!abort).then_some(operation)),
                    );
                }
                _ => return Some(operation.path.to_owned()),
            }
        }
        None
    }

    fn get_copy_path(&self) -> Option<PathBuf> {
        let mut pending = vec![(self, None)];
        while let Some((operation, copy_path)) = pending.pop() {
            match &operation.basic {
                BasicFileOperation::WriteNormalFile { .. }
                | BasicFileOperation::WriteSymlinkWithTarget { .. } => {
                    return Some(copy_path.unwrap_or_else(|| operation.path.clone()));
                }
                BasicFileOperation::WriteDirectory(_)
                | BasicFileOperation::SetArchiveComment(_) => {}
                BasicFileOperation::ShallowCopy(base) | BasicFileOperation::DeepCopy(base) => {
                    pending.push((
                        base,
                        Some(copy_path.unwrap_or_else(|| operation.path.clone())),
                    ));
                }
                BasicFileOperation::MergeWithOtherFile { operations, .. } => {
                    pending.extend(operations.iter().rev().filter_map(|(operation, abort)| {
                        (!abort).then_some((operation, copy_path.clone()))
                    }));
                }
            }
        }
        None
    }

    fn is_streamable(&self) -> bool {
        // The flag records whether the operation runs on a sub-writer that is later merged.
        let mut pending = vec![(self, false)];
        while let Some((operation, in_sub_writer)) = pending.pop() {
            match &operation.basic {
                BasicFileOperation::WriteNormalFile { options, .. }
                | BasicFileOperation::WriteDirectory(options)
                | BasicFileOperation::WriteSymlinkWithTarget { options, .. } => {
                    if options.has_encryption() {
                        return false;
                    }
                }
                BasicFileOperation::ShallowCopy(base) | BasicFileOperation::DeepCopy(base) => {
                    pending.push((base, in_sub_writer));
                }
                BasicFileOperation::MergeWithOtherFile {
                    operations,
                    initial_junk,
                } => {
                    if !initial_junk.is_empty() {
                        return false;
                    }
                    // An aborted file's bytes stay in the stream (see below).
                    if operations.iter().any(|(_, abort)| *abort) {
                        return false;
                    }
                    pending.extend(operations.iter().map(|(operation, _)| (operation, true)));
                }
                // A sub-writer reopened after its comment shrinks can pad the start of its
                // archive with junk (see `finalize_operation`), which `merge_archive` copies in.
                // The top-level writer tracks this for its own comment, but not for sub-writers.
                BasicFileOperation::SetArchiveComment(_) => {
                    if in_sub_writer {
                        return false;
                    }
                }
            }
        }
        true
    }
}

#[derive(Arbitrary, Clone, Debug)]
pub struct FuzzTestCase<'k, 'n> {
    initial_junk: Box<[u8]>,
    operations: Box<[(FileOperation<'k, 'n>, bool)]>,
    flush_on_finish_file: bool,
}

fn deduplicate_paths(
    copy: &mut PathBuf,
    original: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    if path_to_string(&**copy)? == path_to_string(original)? {
        let new_path = match original.file_name() {
            Some(name) => {
                let mut new_name = name.to_owned();
                new_name.push("_copy");
                copy.with_file_name(new_name)
            }
            None => copy.with_file_name("copy"),
        };
        *copy = new_path;
    }
    Ok(())
}

/// Errors that `ZipWriter` returns by design for an arbitrary `FileOptions` or path, rather
/// than because of a bug. Each one is returned before the writer's state changes.
fn is_expected_new_entry_error(e: &ZipError) -> bool {
    match e {
        // `GenericZipWriter::prepare_next_writer` (src/write.rs): the compression method or
        // level isn't available for writing with the enabled features.
        ZipError::UnsupportedArchive(msg) => matches!(
            *msg,
            "Unsupported compression"
                | "Unsupported compression level"
                | "Compressing Deflate64 is not supported"
                | "LZMA isn't supported for compression"
                | "AES encryption is enabled through FileOptions::with_aes_encryption"
                | "Shrink compression unsupported"
                | "Reduce compression unsupported"
                | "Implode compression unsupported"
        ),
        // `ZipWriter::insert_file_data`: two arbitrary paths can normalize to the same name.
        // `ZipWriter::start_entry` with `validate_extra_fields(true)`: the arbitrary options
        // contain an extra field that the reader would reject.
        ZipError::InvalidArchive(msg) => {
            msg.starts_with("Duplicate filename: ")
                || (msg.starts_with("Extra field ")
                    && (msg.ends_with(" would not be readable: malformed data")
                        || msg.ends_with(" would not be readable: renames the entry")))
        }
        _ => false,
    }
}

/// Errors from `deep_copy_file`/`shallow_copy_file` that are expected when the source was never
/// written (an expected error above skipped it) or the destination name is already taken.
fn is_expected_copy_error(e: &ZipError) -> bool {
    match e {
        ZipError::FileNotFound => true,
        ZipError::InvalidArchive(msg) => {
            msg == "That file already exists"
                || msg == "Trying to copy a file to itself"
                || msg.starts_with("Duplicate filename: ")
        }
        _ => is_expected_new_entry_error(e),
    }
}

/// Turns an expected error into `Ok(false)` (logging it to the transcript) so the caller can skip
/// updating its model; any other error is passed through.
fn accept_expected(
    result: ZipResult<()>,
    expected: fn(&ZipError) -> bool,
    stringifier: &mut impl Write,
) -> Result<bool, Box<dyn std::error::Error>> {
    match result {
        Ok(()) => Ok(true),
        Err(e) if expected(&e) => {
            writeln!(stringifier, "// expected error: {e}")?;
            Ok(false)
        }
        Err(e) => Err(e.into()),
    }
}

enum OperationWork<'k, 'n> {
    Run {
        operation: FileOperation<'k, 'n>,
        abort: bool,
        flush_on_finish_file: bool,
        writer_index: usize,
        allow_reopen_failure: bool,
    },
    Copy {
        base_path: PathBuf,
        path: PathBuf,
        deep: bool,
        writer_index: usize,
    },
    Merge {
        parent_index: usize,
    },
    Finalize {
        abort: bool,
        reopen: ReopenOption,
        writer_index: usize,
        allow_reopen_failure: bool,
    },
}

fn finalize_operation(
    writer: &mut ZipWriter<Cursor<Vec<u8>>>,
    abort: bool,
    reopen: ReopenOption,
    files_added: &mut usize,
    stringifier: &mut impl Write,
    allow_reopen_failure: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    if abort && *files_added != 0 {
        writeln!(stringifier, "writer.abort_file()?;")?;
        writer.abort_file()?;
        *files_added -= 1;
    }

    fn try_into_new_writer(
        old_writer: ZipWriter<Cursor<Vec<u8>>>,
    ) -> ZipResult<ZipWriter<Cursor<Vec<u8>>>> {
        ZipWriter::new_append(old_writer.finish()?)
    }

    // Reopening after setting a shorter comment can leave junk after the new comment.
    match reopen {
        ReopenOption::DoNotReopen => writeln!(stringifier, "writer")?,
        ReopenOption::ViaFinish => {
            let old_comment = writer.get_raw_comment().to_owned();
            writeln!(
                stringifier,
                "let mut writer = ZipWriter::new_append(writer.finish()?)?;"
            )?;
            let mut failed_replace = None;
            replace_with_or_abort(writer, |old_writer: ZipWriter<Cursor<Vec<u8>>>| {
                match try_into_new_writer(old_writer) {
                    Ok(writer) => writer,
                    Err(e) => {
                        failed_replace = Some(e);
                        ZipWriter::new(Cursor::new(Vec::new()))
                    }
                }
            });
            if let Some(e) = failed_replace {
                if allow_reopen_failure {
                    return Ok(false);
                }
                return Err(e.into());
            }
            if !writer.get_raw_comment().starts_with(&old_comment) {
                return Err(ZipError::InvalidArchive(
                    "Comment unexpectedly changed after reopening".into(),
                )
                .into());
            }
        }
        ReopenOption::ViaFinishIntoReadable => {
            let old_comment = writer.get_raw_comment().to_owned();
            writeln!(
                stringifier,
                "let mut writer = ZipWriter::new_append(writer.finish_into_readable()?.into_inner())?;"
            )?;
            let mut failed_replace = None;
            replace_with_or_abort(
                writer,
                |old_writer: ZipWriter<Cursor<Vec<u8>>>| match old_writer
                    .finish_into_readable()
                    .and_then(|archive| ZipWriter::new_append(archive.into_inner()))
                {
                    Ok(writer) => writer,
                    Err(e) => {
                        failed_replace = Some(e);
                        ZipWriter::new(Cursor::new(Vec::new()))
                    }
                },
            );
            if let Some(e) = failed_replace {
                if allow_reopen_failure {
                    return Ok(false);
                }
                return Err(e.into());
            }
            if !writer.get_raw_comment().starts_with(&old_comment) {
                return Err(ZipError::InvalidArchive(
                    "Comment unexpectedly changed after reopening".into(),
                )
                .into());
            }
        }
    }
    Ok(true)
}

/// Returns Ok(true) if everything went okay, Ok(false) if an allowed reopen failure occurred, or
/// Err if any other failure occurred.
fn do_operation(
    writer: &mut zip::ZipWriter<Cursor<Vec<u8>>>,
    operation: FileOperation<'_, '_>,
    abort: bool,
    flush_on_finish_file: bool,
    files_added: &mut usize,
    stringifier: &mut impl Write,
    allow_reopen_failure: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    let mut pending = vec![OperationWork::Run {
        operation,
        abort,
        flush_on_finish_file,
        writer_index: 0,
        allow_reopen_failure,
    }];
    let mut sub_writers = Vec::new();
    let mut files_added_by_writer = vec![*files_added];

    while let Some(work) = pending.pop() {
        match work {
            OperationWork::Run {
                operation,
                abort,
                flush_on_finish_file,
                writer_index,
                allow_reopen_failure,
            } => {
                let writer = if writer_index == 0 {
                    &mut *writer
                } else {
                    &mut sub_writers[writer_index - 1]
                };
                writer.set_flush_on_finish_file(flush_on_finish_file);
                let FileOperation {
                    basic,
                    mut path,
                    reopen,
                } = operation;
                let copy = matches!(
                    &basic,
                    BasicFileOperation::DeepCopy(_) | BasicFileOperation::ShallowCopy(_)
                );

                match basic {
                    BasicFileOperation::ShallowCopy(base) | BasicFileOperation::DeepCopy(base) => {
                        let base_path = if copy {
                            base.get_copy_path()
                        } else {
                            base.get_path()
                        };
                        let Some(base_path) = base_path else {
                            continue;
                        };
                        deduplicate_paths(&mut path, &base_path)?;
                        pending.push(OperationWork::Finalize {
                            abort,
                            reopen,
                            writer_index,
                            allow_reopen_failure,
                        });
                        pending.push(OperationWork::Copy {
                            base_path,
                            path,
                            deep: copy,
                            writer_index,
                        });
                        pending.push(OperationWork::Run {
                            operation: *base,
                            abort: false,
                            flush_on_finish_file,
                            writer_index,
                            allow_reopen_failure,
                        });
                    }
                    BasicFileOperation::MergeWithOtherFile {
                        operations,
                        initial_junk,
                    } => {
                        if initial_junk.is_empty() {
                            writeln!(
                                stringifier,
                                "let sub_writer = {{\n\
                                 let mut writer = ZipWriter::new(Cursor::new(Vec::new()));"
                            )?;
                        } else {
                            writeln!(
                                stringifier,
                                "let sub_writer = {{\n\
                                      let mut initial_junk = Cursor::new(vec!{initial_junk:?});\n\
                                      initial_junk.seek(SeekFrom::End(0))?;\n\
                                      let mut writer = ZipWriter::new(initial_junk);",
                            )?;
                        }
                        let mut initial_junk = Cursor::new(initial_junk.into_vec());
                        initial_junk.seek(SeekFrom::End(0))?;
                        sub_writers.push(ZipWriter::new(initial_junk));
                        files_added_by_writer.push(0);

                        pending.push(OperationWork::Finalize {
                            abort,
                            reopen,
                            writer_index,
                            allow_reopen_failure,
                        });
                        pending.push(OperationWork::Merge {
                            parent_index: writer_index,
                        });
                        pending.extend(operations.into_vec().into_iter().rev().map(
                            |(operation, abort)| OperationWork::Run {
                                operation,
                                abort,
                                flush_on_finish_file: false,
                                writer_index: writer_index + 1,
                                allow_reopen_failure,
                            },
                        ));
                    }
                    basic => {
                        pending.push(OperationWork::Finalize {
                            abort,
                            reopen,
                            writer_index,
                            allow_reopen_failure,
                        });
                        let writer = if writer_index == 0 {
                            &mut *writer
                        } else {
                            &mut sub_writers[writer_index - 1]
                        };
                        match basic {
                            BasicFileOperation::WriteNormalFile { contents, options } => {
                                let mut options = options.validate_extra_fields(true);
                                let uncompressed_size =
                                    contents.iter().map(|chunk| chunk.len()).sum::<usize>();
                                if uncompressed_size >= u32::MAX as usize {
                                    options = options.large_file(true);
                                }
                                if options == FullFileOptions::default().validate_extra_fields(true)
                                {
                                    writeln!(
                                        stringifier,
                                        "writer.start_file_from_path({path:?}, \
                                         FullFileOptions::default().validate_extra_fields(true))?;"
                                    )?;
                                } else {
                                    writeln!(
                                        stringifier,
                                        "writer.start_file_from_path({path:?}, {options:?})?;"
                                    )?;
                                }
                                if !accept_expected(
                                    writer.start_file_from_path(&*path, options),
                                    is_expected_new_entry_error,
                                    stringifier,
                                )? {
                                    continue;
                                }
                                for chunk in contents.iter() {
                                    writeln!(stringifier, "writer.write_all(&{chunk:?})?;")?;
                                    writer.write_all(chunk)?;
                                }
                                files_added_by_writer[writer_index] += 1;
                            }
                            BasicFileOperation::WriteDirectory(options) => {
                                let options = options.validate_extra_fields(true);
                                writeln!(
                                    stringifier,
                                    "writer.add_directory_from_path(&{path:?}, {options:?})?;"
                                )?;
                                if !accept_expected(
                                    writer.add_directory_from_path(&*path, options),
                                    is_expected_new_entry_error,
                                    stringifier,
                                )? {
                                    continue;
                                }
                                files_added_by_writer[writer_index] += 1;
                            }
                            BasicFileOperation::WriteSymlinkWithTarget { target, options } => {
                                let options = options.validate_extra_fields(true);
                                writeln!(
                                    stringifier,
                                    "writer.add_symlink_from_path(&{path:?}, {target:?}, {options:?});"
                                )?;
                                if !accept_expected(
                                    writer.add_symlink_from_path(&*path, target, options),
                                    is_expected_new_entry_error,
                                    stringifier,
                                )? {
                                    continue;
                                }
                                files_added_by_writer[writer_index] += 1;
                            }
                            BasicFileOperation::SetArchiveComment(comment) => {
                                writeln!(stringifier, "writer.set_raw_comment({comment:?})?;")?;
                                writer.set_raw_comment(comment)?;
                            }
                            BasicFileOperation::ShallowCopy(_)
                            | BasicFileOperation::DeepCopy(_)
                            | BasicFileOperation::MergeWithOtherFile { .. } => unreachable!(),
                        }
                    }
                }
            }
            OperationWork::Copy {
                base_path,
                path,
                deep,
                writer_index,
            } => {
                let writer = if writer_index == 0 {
                    &mut *writer
                } else {
                    &mut sub_writers[writer_index - 1]
                };
                if deep {
                    writeln!(
                        stringifier,
                        "writer.deep_copy_file_from_path({base_path:?}, {path:?});"
                    )?;
                    if !accept_expected(
                        writer.deep_copy_file_from_path(&*base_path, path),
                        is_expected_copy_error,
                        stringifier,
                    )? {
                        continue;
                    }
                } else {
                    writeln!(
                        stringifier,
                        "writer.shallow_copy_file_from_path({base_path:?}, {path:?});"
                    )?;
                    if !accept_expected(
                        writer.shallow_copy_file_from_path(&*base_path, &*path),
                        is_expected_copy_error,
                        stringifier,
                    )? {
                        continue;
                    }
                }
                files_added_by_writer[writer_index] += 1;
            }
            OperationWork::Merge { parent_index } => {
                writeln!(
                    stringifier,
                    "writer\n}};\nwriter.merge_archive(sub_writer.finish_into_readable()?)?"
                )?;
                let other_writer = sub_writers.pop().expect("merge writer was pushed");
                let inner_files_added =
                    files_added_by_writer.pop().expect("merge count was pushed");
                let parent_writer = if parent_index == 0 {
                    &mut *writer
                } else {
                    &mut sub_writers[parent_index - 1]
                };
                // Both archives can contain the same (normalized) name.
                if accept_expected(
                    parent_writer.merge_archive(other_writer.finish_into_readable()?),
                    is_expected_new_entry_error,
                    stringifier,
                )? {
                    files_added_by_writer[parent_index] += inner_files_added;
                }
            }
            OperationWork::Finalize {
                abort,
                reopen,
                writer_index,
                allow_reopen_failure,
            } => {
                let writer = if writer_index == 0 {
                    &mut *writer
                } else {
                    &mut sub_writers[writer_index - 1]
                };
                if !finalize_operation(
                    writer,
                    abort,
                    reopen,
                    &mut files_added_by_writer[writer_index],
                    stringifier,
                    allow_reopen_failure,
                )? {
                    return Ok(false);
                }
            }
        }
    }

    *files_added = files_added_by_writer[0];
    Ok(true)
}

impl FuzzTestCase<'_, '_> {
    fn execute<W: Write>(self, mut stringifier: impl ops::DerefMut<Target = W>) -> ZipResult<()> {
        // Indicates the starting position if we use read_zipfile_from_stream at the end.
        let junk_len = self.initial_junk.len();

        let mut initial_junk = Cursor::new(self.initial_junk.into_vec());
        initial_junk.seek(SeekFrom::End(0))?;
        let mut writer = zip::ZipWriter::new(initial_junk);
        let mut files_added = 0;
        let mut final_reopen = false;
        if let Some((last_op, _)) = self.operations.last()
            && last_op.reopen != ReopenOption::ViaFinishIntoReadable
        {
            final_reopen = true;
        }
        let mut streamable = true;

        // If the comment is set, the archive is reopened and then a smaller comment is set, there
        // may be more leading junk than planned, so streaming reading may produce an error. But it
        // still shouldn't panic, and ZipArchive::new should still work.
        let mut comment_len = 0;
        let mut comment_len_at_reopen = 0;

        // If the comment contains an EOCD or EOCD64, opening the archive for read may return an error,
        // but still shouldn't panic.
        let mut comment_contains_signature = false;

        #[allow(unknown_lints)]
        #[allow(boxed_slice_into_iter)]
        for (operation, abort) in self.operations.into_vec().into_iter() {
            // `abort_file` can't truncate the inner writer, so an aborted file's bytes stay where a
            // local header is expected and `finalize` moves the footer past them; only the central
            // directory (`ZipArchive::new`) can be relied on afterwards.
            if streamable && (abort || !operation.is_streamable()) {
                streamable = false;
            }
            // An empty comment counts too: it's the shortest possible one after a reopen.
            if let BasicFileOperation::SetArchiveComment(ref comment) = operation.basic {
                comment_len = comment.len();
                if !comment_contains_signature {
                    if comment_len >= 22
                        && comment[0..=(comment_len - 18)]
                            .windows(4)
                            .any(|w| w == b"PK\x05\x06")
                    {
                        // ZIP32 EOCD
                        comment_contains_signature = true;
                    } else if comment_len >= 56
                        && comment[0..=(comment_len - 52)]
                            .windows(4)
                            .any(|w| w == b"PK\x06\x06")
                    {
                        // ZIP64 EOCD
                        comment_contains_signature = true;
                    } else if comment_len >= 20
                        && comment[0..=(comment_len - 16)]
                            .windows(4)
                            .any(|w| w == b"PK\x06\x07")
                    {
                        // ZIP64 EOCD locator
                        comment_contains_signature = true;
                    }
                }
                if streamable && comment_len_at_reopen > comment_len {
                    streamable = false;
                }
            }
            let reopen = operation.reopen;
            if reopen != ReopenOption::DoNotReopen {
                comment_len_at_reopen = comment_len;
            }
            let allow_reopen_failure =
                comment_contains_signature && reopen == ReopenOption::ViaFinish;
            let completed = do_operation(
                &mut writer,
                operation,
                abort,
                self.flush_on_finish_file,
                &mut files_added,
                stringifier.deref_mut(),
                allow_reopen_failure,
            )
            .map_err(|e| ZipError::InvalidArchive(format!("Error during operation: {e}").into()))?;
            if !completed {
                return Ok(());
            }
        }
        if final_reopen {
            writeln!(stringifier, "let _ = writer.finish_into_readable()?;")
                .map_err(|_| ZipError::InvalidArchive("".into()))?;
            let _ = writer.finish_into_readable()?;
        } else {
            writeln!(
                stringifier,
                "let mut stream = writer.finish()?;\n\
                    stream.seek(SeekFrom::Start({junk_len}))?;\n"
            )
            .unwrap();
            if streamable {
                writeln!(
                    stringifier,
                    "while read_zipfile_from_stream(&mut stream)?.is_some() {{}}\n"
                )
                .unwrap();
            } else {
                writeln!(
                    stringifier,
                    "while matches!(read_zipfile_from_stream(&mut stream), Ok(Some(_))) {{}}\n"
                )
                .unwrap();
            }
            let mut stream = writer.finish()?;
            stream.seek(SeekFrom::Start(junk_len as u64))?;
            loop {
                match read_zipfile_from_stream(&mut stream) {
                    Ok(None) => break,
                    Ok(Some(mut file)) => {
                        let _ = copy(&mut file, &mut sink())?;
                    }
                    Err(e) => {
                        if streamable {
                            return Err(e);
                        } else {
                            break;
                        }
                    }
                }
            }
            if !comment_contains_signature {
                writeln!(stringifier, "let _ = ZipArchive::new(stream)?\n").unwrap();
                let _ = ZipArchive::new(stream)?;
            }
        }
        Ok(())
    }
}

fn main() {
    #[cfg(fuzzing)]
    unsafe {
        backtrace_on_stack_overflow::enable()
    };

    use std::io::Read;
    let mut v = Vec::new();
    std::io::stdin().read_to_end(&mut v).unwrap();
    let u = Unstructured::new(&v[..]);
    let mut w = String::new();
    if let Ok(it) = u.arbitrary_take_rest_iter::<FuzzTestCase>() {
        for test_case in it.flatten() {
            w.clear();
            let result = test_case.execute(&mut w);
            assert!(
                result.is_ok(),
                "Test case failed:\n{}\n{}",
                w,
                result.err().unwrap()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_operation(
        basic: BasicFileOperation<'static, 'static>,
        path: impl Into<PathBuf>,
    ) -> FileOperation<'static, 'static> {
        FileOperation {
            basic,
            path: path.into(),
            reopen: ReopenOption::DoNotReopen,
        }
    }

    fn write_file(path: impl Into<PathBuf>) -> FileOperation<'static, 'static> {
        file_operation(
            BasicFileOperation::WriteNormalFile {
                contents: Vec::new().into_boxed_slice(),
                options: FullFileOptions::default(),
            },
            path,
        )
    }

    #[test]
    fn deeply_nested_copy_operations_use_a_heap_work_stack() {
        let mut operation = write_file("base");
        for depth in 0..4096 {
            let basic = if depth % 2 == 0 {
                BasicFileOperation::ShallowCopy(Box::new(operation))
            } else {
                BasicFileOperation::DeepCopy(Box::new(operation))
            };
            operation = file_operation(basic, format!("copy-{depth}"));
        }

        assert!(operation.is_streamable());
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let mut files_added = 0;
        do_operation(
            &mut writer,
            operation,
            false,
            false,
            &mut files_added,
            &mut String::new(),
            false,
        )
        .unwrap();

        let archive = writer.finish_into_readable().unwrap();
        assert_eq!(archive.len(), 4097);
    }

    #[test]
    fn deeply_nested_merges_use_a_heap_work_stack() {
        let mut operation = write_file("base");
        for _ in 0..2048 {
            operation = file_operation(
                BasicFileOperation::MergeWithOtherFile {
                    initial_junk: Vec::new().into_boxed_slice(),
                    operations: vec![(operation, false)].into_boxed_slice(),
                },
                "merged",
            );
        }

        assert!(operation.is_streamable());
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let mut files_added = 0;
        do_operation(
            &mut writer,
            operation,
            false,
            false,
            &mut files_added,
            &mut String::new(),
            false,
        )
        .unwrap();

        let archive = writer.finish_into_readable().unwrap();
        assert_eq!(archive.len(), 1);
    }

    #[test]
    fn deep_copy_skips_operations_that_do_not_write_a_file() {
        let operation = file_operation(
            BasicFileOperation::DeepCopy(Box::new(file_operation(
                BasicFileOperation::WriteDirectory(FullFileOptions::default()),
                "",
            ))),
            "copy",
        );
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let mut files_added = 0;
        do_operation(
            &mut writer,
            operation,
            false,
            false,
            &mut files_added,
            &mut String::new(),
            false,
        )
        .unwrap();

        let archive = writer.finish_into_readable().unwrap();
        assert_eq!(archive.len(), 0);
    }

    #[test]
    fn shallow_copy_skips_operations_that_do_not_write_a_file() {
        let operation = file_operation(
            BasicFileOperation::ShallowCopy(Box::new(file_operation(
                BasicFileOperation::WriteDirectory(FullFileOptions::default()),
                "",
            ))),
            "copy",
        );
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let mut files_added = 0;
        do_operation(
            &mut writer,
            operation,
            false,
            false,
            &mut files_added,
            &mut String::new(),
            false,
        )
        .unwrap();

        let archive = writer.finish_into_readable().unwrap();
        assert_eq!(archive.len(), 0);
    }
}
