#![allow(unexpected_cfgs)] // Needed for cfg(fuzzing) on nightly as of 2024-05-06

use arbitrary::{Arbitrary, Unstructured};
use core::fmt::Debug;
use replace_with::replace_with_or_abort;
use std::fmt::{Formatter, Write};
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

#[derive(Arbitrary, Clone)]
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

#[derive(Arbitrary, Clone)]
pub struct FileOperation<'k, 'n> {
    basic: BasicFileOperation<'k, 'n>,
    path: PathBuf,
    reopen: ReopenOption,
    // 'abort' flag is separate, to prevent trying to copy an aborted file
}

impl FileOperation<'_, '_> {
    fn get_path(&self) -> Option<PathBuf> {
        match &self.basic {
            BasicFileOperation::SetArchiveComment(_) => None,
            BasicFileOperation::WriteDirectory(_) => Some(self.path.join("")),
            BasicFileOperation::MergeWithOtherFile { operations, .. } => operations
                .iter()
                .flat_map(|(op, abort)| if !abort { op.get_path() } else { None })
                .next(),
            _ => Some(self.path.to_owned()),
        }
    }

    fn is_streamable(&self) -> bool {
        match &self.basic {
            BasicFileOperation::WriteNormalFile { options, .. }
            | BasicFileOperation::WriteDirectory(options)
            | BasicFileOperation::WriteSymlinkWithTarget { options, .. } => {
                !options.has_encryption()
            }
            BasicFileOperation::ShallowCopy(base) => base.is_streamable(),
            BasicFileOperation::DeepCopy(base) => base.is_streamable(),
            BasicFileOperation::MergeWithOtherFile {
                operations,
                initial_junk,
            } => {
                if !initial_junk.is_empty() {
                    return false;
                }
                operations.iter().all(|(op, _)| op.is_streamable())
            }
            _ => true,
        }
    }
}

#[derive(Arbitrary, Clone)]
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
    writer.set_flush_on_finish_file(flush_on_finish_file);
    let FileOperation {
        basic,
        mut path,
        reopen,
    } = operation;
    match basic {
        BasicFileOperation::WriteNormalFile {
            contents,
            mut options,
            ..
        } => {
            let uncompressed_size = contents.iter().map(|chunk| chunk.len()).sum::<usize>();
            if uncompressed_size >= u32::MAX as usize {
                options = options.large_file(true);
            }
            if options == FullFileOptions::default() {
                writeln!(
                    stringifier,
                    "writer.start_file_from_path({path:?}, Default::default())?;",
                )?;
            } else {
                writeln!(
                    stringifier,
                    "writer.start_file_from_path({path:?}, {options:?})?;",
                )?;
            }
            writer.start_file_from_path(&*path, options)?;
            for chunk in contents.iter() {
                writeln!(stringifier, "writer.write_all(&{chunk:?})?;")?;
                writer.write_all(chunk)?;
            }
            *files_added += 1;
        }
        BasicFileOperation::WriteDirectory(options) => {
            writeln!(
                stringifier,
                "writer.add_directory_from_path(&{path:?}, {options:?})?;",
            )?;
            writer.add_directory_from_path(&*path, options.to_owned())?;
            *files_added += 1;
        }
        BasicFileOperation::WriteSymlinkWithTarget { target, options } => {
            writeln!(
                stringifier,
                "writer.add_symlink_from_path(&{path:?}, {target:?}, {options:?});",
            )?;
            writer.add_symlink_from_path(&*path, target, options.to_owned())?;
            *files_added += 1;
        }
        BasicFileOperation::ShallowCopy(base) => {
            let Some(base_path) = base.get_path() else {
                return Ok(true);
            };
            deduplicate_paths(&mut path, &base_path)?;
            if !do_operation(
                writer,
                *base,
                false,
                flush_on_finish_file,
                files_added,
                stringifier,
                allow_reopen_failure,
            )? {
                return Ok(false);
            }
            writeln!(
                stringifier,
                "writer.shallow_copy_file_from_path({base_path:?}, {path:?});",
            )?;
            writer.shallow_copy_file_from_path(&*base_path, &*path)?;
            *files_added += 1;
        }
        BasicFileOperation::DeepCopy(base) => {
            let Some(base_path) = base.get_path() else {
                return Ok(true);
            };
            deduplicate_paths(&mut path, &base_path)?;
            if !do_operation(
                writer,
                *base,
                false,
                flush_on_finish_file,
                files_added,
                stringifier,
                allow_reopen_failure,
            )? {
                return Ok(false);
            }
            writeln!(
                stringifier,
                "writer.deep_copy_file_from_path({base_path:?}, {path:?});",
            )?;
            writer.deep_copy_file_from_path(&*base_path, path)?;
            *files_added += 1;
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
                          initial_junk.seek(SeekFrom::End(0))?;
                          let mut writer = ZipWriter::new(initial_junk);",
                )?;
            }
            let mut initial_junk = Cursor::new(initial_junk.into_vec());
            initial_junk.seek(SeekFrom::End(0))?;
            let mut other_writer = zip::ZipWriter::new(initial_junk);
            let mut inner_files_added = 0;
            for (operation, abort) in operations {
                let completed = do_operation(
                        &mut other_writer,
                        operation,
                        abort,
                        false,
                        &mut inner_files_added,
                        stringifier,
                        allow_reopen_failure,
                    )?;
                if !completed {
                    return Ok(false);
                }
            }
            writeln!(
                stringifier,
                "writer\n}};\nwriter.merge_archive(sub_writer.finish_into_readable()?)?;"
            )?;
            writer.merge_archive(other_writer.finish_into_readable()?)?;
            *files_added += inner_files_added;
        }
        BasicFileOperation::SetArchiveComment(comment) => {
            writeln!(stringifier, "writer.set_raw_comment({comment:?})?;")?;
            writer.set_raw_comment(comment.clone())?;
        }
    }
    if abort && *files_added != 0 {
        writeln!(stringifier, "writer.abort_file()?;")?;
        writer.abort_file()?;
        *files_added -= 1;
    }
    fn try_into_new_writer(
        old_writer: zip::ZipWriter<Cursor<Vec<u8>>>,
    ) -> ZipResult<zip::ZipWriter<Cursor<Vec<u8>>>> {
        zip::ZipWriter::new_append(old_writer.finish()?)
    }
    // If a comment is set, we finish the archive, reopen it for append and then set a shorter
    // comment, then there will be junk after the new comment that we can't get rid of. Thus, we
    // can only check that the expected is a prefix of the actual
    match reopen {
        ReopenOption::DoNotReopen => {
            writeln!(stringifier, "writer")?;
            return Ok(true);
        }
        ReopenOption::ViaFinish => {
            let old_comment = writer.get_raw_comment().to_owned();
            writeln!(
                stringifier,
                "let mut writer = ZipWriter::new_append(writer.finish()?)?;"
            )?;
            let mut failed_replace = None;
            replace_with_or_abort(writer, |old_writer: zip::ZipWriter<Cursor<Vec<u8>>>| {
                match try_into_new_writer(old_writer) {
                    Ok(writer) => writer,
                    Err(e) => {
                        failed_replace = Some(e);
                        zip::ZipWriter::new(Cursor::new(Vec::new()))
                    }
                }
            });
            if !allow_reopen_failure && let Some(e) = failed_replace {
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
                "let mut writer = ZipWriter::new_append(writer.finish()?)?;"
            )?;
            let mut failed_replace = None;
            replace_with_or_abort(writer, |old_writer: zip::ZipWriter<Cursor<Vec<u8>>>| {
                match old_writer
                    .finish_into_readable()
                    .map(|archive| ZipWriter::new(archive.into_inner()))
                {
                    Ok(writer) => writer,
                    Err(e) => {
                        failed_replace = Some(e);
                        zip::ZipWriter::new(Cursor::new(Vec::new()))
                    }
                }
            });
            if !allow_reopen_failure && let Some(e) = failed_replace {
                return Err(e.into());
            }
            debug_assert!(writer.get_raw_comment().starts_with(&old_comment));
        }
    }
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
            if streamable && !operation.is_streamable() {
                streamable = false;
            }
            if let BasicFileOperation::SetArchiveComment(ref comment) = operation.basic
                && !comment.is_empty()
            {
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
            let allow_reopen_failure = comment_contains_signature && reopen == ReopenOption::ViaFinish;
            let completed = do_operation(
                &mut writer,
                operation,
                abort,
                self.flush_on_finish_file,
                &mut files_added,
                stringifier.deref_mut(),
                allow_reopen_failure
            ).map_err(|e| ZipError::InvalidArchive(format!("Error during operation: {e}").into()))?;
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

impl Debug for FuzzTestCase<'_, '_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.initial_junk.is_empty() {
            writeln!(
                f,
                "let mut writer = ZipWriter::new(Cursor::new(Vec::new()));"
            )?;
        } else {
            writeln!(
                f,
                "let mut initial_junk = Cursor::new(vec!{:?});\n\
                         initial_junk.seek(SeekFrom::End(0))?;\n\
                         let mut writer = ZipWriter::new(initial_junk);",
                self.initial_junk
            )?;
        }
        let _ = self.clone().execute(f);
        Ok(())
    }
}

fn main() {
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
