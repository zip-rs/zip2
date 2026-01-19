#![allow(unexpected_cfgs)] // Needed for cfg(fuzzing) on nightly as of 2024-05-06

#[cfg(fuzzing)]
use afl::fuzz;
use arbitrary::{Arbitrary, Unstructured};
use core::fmt::Debug;
use replace_with::replace_with_or_abort;
use std::fmt::{Formatter, Write};
use std::io::Write as IoWrite;
use std::io::{Cursor, Seek, SeekFrom};
use std::ops;
use std::path::PathBuf;
#[cfg(fuzzing)]
use tikv_jemallocator::Jemalloc;
use zip::read::read_zipfile_from_stream;
use zip::result::{ZipError, ZipResult};
use zip::unstable::path_to_string;
use zip::write::FullFileOptions;

#[cfg(fuzzing)]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

#[derive(Arbitrary, Clone)]
pub enum BasicFileOperation<'k> {
    WriteNormalFile {
        contents: Box<[Box<[u8]>]>,
        options: FullFileOptions<'k>,
    },
    WriteDirectory(FullFileOptions<'k>),
    WriteSymlinkWithTarget {
        target: PathBuf,
        options: FullFileOptions<'k>,
    },
    ShallowCopy(Box<FileOperation<'k>>),
    DeepCopy(Box<FileOperation<'k>>),
    MergeWithOtherFile {
        initial_junk: Box<[u8]>,
        operations: Box<[(FileOperation<'k>, bool)]>,
    },
    SetArchiveComment(Box<[u8]>),
}

#[derive(Arbitrary, Clone, Debug, Eq, PartialEq)]
pub enum ReopenOption {
    DoNotReopen,
    ViaFinish,
    ViaFinishIntoReadable,
}

#[derive(Arbitrary, Clone)]
pub struct FileOperation<'k> {
    basic: BasicFileOperation<'k>,
    path: PathBuf,
    reopen: ReopenOption,
    // 'abort' flag is separate, to prevent trying to copy an aborted file
}

impl FileOperation<'_> {
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
pub struct FuzzTestCase<'k> {
    initial_junk: Box<[u8]>,
    operations: Box<[(FileOperation<'k>, bool)]>,
    flush_on_finish_file: bool,
}

fn deduplicate_paths(copy: &mut PathBuf, original: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
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

fn do_operation(
    writer: &mut zip::ZipWriter<Cursor<Vec<u8>>>,
    operation: FileOperation<'_>,
    abort: bool,
    flush_on_finish_file: bool,
    files_added: &mut usize,
    stringifier: &mut impl Write,
    panic_on_error: bool,
) -> Result<(), Box<dyn std::error::Error>> {
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
                return Ok(());
            };
            deduplicate_paths(&mut path, &base_path)?;
            do_operation(
                writer,
                *base,
                false,
                flush_on_finish_file,
                files_added,
                stringifier,
                panic_on_error,
            )?;
            writeln!(
                stringifier,
                "writer.shallow_copy_file_from_path({base_path:?}, {path:?});",
            )?;
            writer.shallow_copy_file_from_path(&*base_path, &*path)?;
            *files_added += 1;
        }
        BasicFileOperation::DeepCopy(base) => {
            let Some(base_path) = base.get_path() else {
                return Ok(());
            };
            deduplicate_paths(&mut path, &base_path)?;
            do_operation(
                writer,
                *base,
                false,
                flush_on_finish_file,
                files_added,
                stringifier,
                panic_on_error,
            )?;
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
            operations
                .into_vec()
                .into_iter()
                .for_each(|(operation, abort)| {
                    let _ = do_operation(
                        &mut other_writer,
                        operation,
                        abort,
                        false,
                        &mut inner_files_added,
                        stringifier,
                        panic_on_error,
                    );
                });
            writeln!(
                stringifier,
                "writer\n}};\nwriter.merge_archive(sub_writer.finish_into_readable()?)?;"
            )?;
            writer.merge_archive(other_writer.finish_into_readable()?)?;
            *files_added += inner_files_added;
        }
        BasicFileOperation::SetArchiveComment(comment) => {
            writeln!(stringifier, "writer.set_raw_comment({comment:?})?;")?;
            writer.set_raw_comment(comment.clone());
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
            return Ok(());
        }
        ReopenOption::ViaFinish => {
            let old_comment = writer.get_raw_comment().to_owned();
            writeln!(
                stringifier,
                "let mut writer = ZipWriter::new_append(writer.finish()?)?;"
            )?;
            replace_with_or_abort(writer, |old_writer: zip::ZipWriter<Cursor<Vec<u8>>>| {
                try_into_new_writer(old_writer).unwrap_or_else(|_| {
                    if panic_on_error {
                        panic!("Failed to create new ZipWriter")
                    }
                    zip::ZipWriter::new(Cursor::new(Vec::new()))
                })
            });
            if panic_on_error {
                assert!(writer.get_raw_comment().starts_with(&old_comment));
            }
        }
        ReopenOption::ViaFinishIntoReadable => {
            let old_comment = writer.get_raw_comment().to_owned();
            writeln!(
                stringifier,
                "let mut writer = ZipWriter::new_append(writer.finish()?)?;"
            )?;
            replace_with_or_abort(writer, |old_writer| {
                try_into_new_writer(old_writer).unwrap_or_else(|_| {
                    if panic_on_error {
                        panic!("Failed to create new ZipWriter")
                    }
                    zip::ZipWriter::new(Cursor::new(Vec::new()))
                })
            });
            debug_assert!(writer.get_raw_comment().starts_with(&old_comment));
        }
    }
    Ok(())
}

impl FuzzTestCase<'_> {
    fn execute<W: Write>(
        self,
        mut stringifier: impl ops::DerefMut<Target = W>,
        panic_on_error: bool,
    ) -> ZipResult<()> {
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
        let streamable = self.operations.iter().all(|(op, _)| op.is_streamable());
        #[allow(unknown_lints)]
        #[allow(boxed_slice_into_iter)]
        for (operation, abort) in self.operations.into_vec().into_iter() {
            let _ = do_operation(
                &mut writer,
                operation,
                abort,
                self.flush_on_finish_file,
                &mut files_added,
                stringifier.deref_mut(),
                panic_on_error,
            );
        }
        if streamable {
            writeln!(
                stringifier,
                "let mut stream = writer.finish()?;\n\
                    stream.seek(SeekFrom::Start({junk_len}))?;\n\
                    while read_zipfile_from_stream(&mut stream)?.is_some() {{}}"
            )
            .map_err(|_| ZipError::InvalidArchive("Failed to read from stream".into()))?;
            let mut stream = writer.finish()?;
            stream.seek(SeekFrom::Start(junk_len as u64))?;
            while read_zipfile_from_stream(&mut stream)?.is_some() {}
        } else if final_reopen {
            writeln!(stringifier, "let _ = writer.finish_into_readable()?;")
                .map_err(|_| ZipError::InvalidArchive("".into()))?;
            let _ = writer.finish_into_readable()?;
        }
        Ok(())
    }
}

impl Debug for FuzzTestCase<'_> {
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
                &self.initial_junk
            )?;
        }
        let _ = self.clone().execute(f, false);
        Ok(())
    }
}

#[cfg(fuzzing)]
#[derive(Default, Eq, PartialEq)]
struct NoopWrite {}

#[cfg(fuzzing)]
impl Write for NoopWrite {
    fn write_str(&mut self, _: &str) -> std::fmt::Result {
        Ok(())
    }

    fn write_char(&mut self, _: char) -> std::fmt::Result {
        Ok(())
    }

    fn write_fmt(&mut self, _: std::fmt::Arguments<'_>) -> std::fmt::Result {
        Ok(())
    }
}

#[cfg(not(fuzzing))]
struct StdoutWrite(std::io::Stdout);

#[cfg(not(fuzzing))]
impl Default for StdoutWrite {
    fn default() -> Self {
        Self(std::io::stdout())
    }
}

#[cfg(not(fuzzing))]
impl Write for StdoutWrite {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        match self.0.write_all(s.as_bytes()) {
            Ok(()) => Ok(()),
            Err(e) => {
                eprintln!("error writing string {s:?}: {e}");
                Err(std::fmt::Error)
            }
        }
    }
}

#[cfg(not(fuzzing))]
impl ops::Drop for StdoutWrite {
    fn drop(&mut self) {
        match self.0.flush() {
            Ok(()) => (),
            Err(e) => {
                eprintln!("error flushing: {e}");
            }
        }
    }
}

fn main() {
    #[cfg(fuzzing)]
    {
        let mut w = NoopWrite::default();
        fuzz!(|data: &[u8]| {
            let u = Unstructured::new(data);
            if let Ok(it) = u.arbitrary_take_rest_iter::<FuzzTestCase>() {
                for test_case in it.flatten() {
                    test_case.execute(&mut w, true).unwrap();
                }
            }
        });
    }

    #[cfg(not(fuzzing))]
    {
        use std::io::Read;
        let mut v = Vec::new();
        std::io::stdin().read_to_end(&mut v).unwrap();
        let u = Unstructured::new(&v[..]);
        let mut w = StdoutWrite::default();
        if let Ok(it) = u.arbitrary_take_rest_iter::<FuzzTestCase>() {
            for test_case in it.flatten() {
                test_case.execute(&mut w, true).unwrap();
            }
        }
    }
}
