#![no_main]

use arbitrary::Arbitrary;
use core::fmt::{Debug};
use libfuzzer_sys::fuzz_target;
use replace_with::replace_with_or_abort;
use std::borrow::Cow;
use std::fmt::Arguments;
use std::io::{Cursor, Read, Seek, Write};
use std::path::PathBuf;
use tikv_jemallocator::Jemalloc;
use zip::result::{ZipError, ZipResult};
use zip::unstable::path_to_string;
use zip::write::FullFileOptions;

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

#[derive(Arbitrary, Clone)]
pub enum BasicFileOperation<'k> {
    WriteNormalFile {
        contents: Box<[Box<[u8]>]>,
        options: zip::write::FullFileOptions<'k>,
    },
    WriteDirectory(zip::write::FullFileOptions<'k>),
    WriteSymlinkWithTarget {
        target: PathBuf,
        options: zip::write::FullFileOptions<'k>,
    },
    ShallowCopy(Box<FileOperation<'k>>),
    DeepCopy(Box<FileOperation<'k>>),
    MergeWithOtherFile {
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

impl<'k> FileOperation<'k> {
    fn get_path(&self) -> Option<Cow<PathBuf>> {
        match &self.basic {
            BasicFileOperation::SetArchiveComment(_) => None,
            BasicFileOperation::WriteDirectory(_) => Some(Cow::Owned(self.path.join("/"))),
            BasicFileOperation::MergeWithOtherFile { operations } => operations
                .iter()
                .flat_map(|(op, abort)| if !abort { op.get_path() } else { None })
                .next(),
            _ => Some(Cow::Borrowed(&self.path)),
        }
    }
}

#[derive(Arbitrary, Clone)]
pub struct FuzzTestCase<'k> {
    operations: Box<[(FileOperation<'k>, bool)]>,
    flush_on_finish_file: bool,
}

fn deduplicate_paths(copy: &mut Cow<PathBuf>, original: &PathBuf) {
    if path_to_string(&**copy) == path_to_string(original) {
        let new_path = match original.file_name() {
            Some(name) => {
                let mut new_name = name.to_owned();
                new_name.push("_copy");
                copy.with_file_name(new_name)
            }
            None => copy.with_file_name("copy"),
        };
        *copy = Cow::Owned(new_path);
    }
}

fn do_operation<'k, T>(
    writer: &mut zip::ZipWriter<T>,
    operation: &FileOperation<'k>,
    abort: bool,
    flush_on_finish_file: bool,
    files_added: &mut usize,
    stringifier: &mut impl std::fmt::Write
) -> Result<(), Box<dyn std::error::Error>>
where
    T: Read + Write + Seek,
{
    writer.set_flush_on_finish_file(flush_on_finish_file);
    let mut path = Cow::Borrowed(&operation.path);
    match &operation.basic {
        BasicFileOperation::WriteNormalFile {
            contents, options, ..
        } => {
            let uncompressed_size = contents.iter().map(|chunk| chunk.len()).sum::<usize>();
            let mut options = (*options).to_owned();
            if uncompressed_size >= u32::MAX as usize {
                options = options.large_file(true);
            }
            if let Some(ref mut stringifier) = stringifier {
                if options == FullFileOptions::default() {
                    writeln!(stringifier, "writer.start_file_from_path({:?}, Default::default())?;", path)?;
                } else {
                    writeln!(stringifier, "writer.start_file_from_path({:?}, {:?})?;", path, options)?;
                }

            }
            writer.start_file_from_path(&*path, options)?;
            for chunk in contents.iter() {
                if let Some(ref mut stringifier) = stringifier {
                    writeln!(stringifier, "writer.write_all(&{:?})?;", chunk)?;
                }
                writer.write_all(&chunk)?;
            }
            *files_added += 1;
        }
        BasicFileOperation::WriteDirectory(options) => {
            if let Some(ref mut stringifier) = stringifier {
                writeln!(stringifier, "writer.add_directory_from_path(&{:?}, {:?})?;", path, options)?;
            }
            writer.add_directory_from_path(&*path, options.to_owned())?;
            *files_added += 1;
        }
        BasicFileOperation::WriteSymlinkWithTarget { target, options } => {
            if let Some(ref mut stringifier) = stringifier {
                writeln!(stringifier, "writer.add_symlink_from_path(&{:?}, {:?}, {:?});", path, target, options)?;
            }
            writer.add_symlink_from_path(&*path, target, options.to_owned())?;
            *files_added += 1;
        }
        BasicFileOperation::ShallowCopy(base) => {
            let Some(base_path) = base.get_path() else {
                return Ok(());
            };
            deduplicate_paths(&mut path, &base_path);
            do_operation(writer, &base, false, flush_on_finish_file, files_added, stringifier)?;
            if let Some(ref mut stringifier) = stringifier {
                writeln!(stringifier, "writer.shallow_copy_file_from_path({:?}, {:?});", base_path, path)?;
            }
            writer.shallow_copy_file_from_path(&*base_path, &*path)?;
            *files_added += 1;
        }
        BasicFileOperation::DeepCopy(base) => {
            let Some(base_path) = base.get_path() else {
                return Ok(());
            };
            deduplicate_paths(&mut path, &base_path);
            do_operation(writer, &base, false, flush_on_finish_file, files_added, stringifier)?;
            if let Some(ref mut stringifier) = stringifier {
                writeln!(stringifier, "writer.deep_copy_file_from_path({:?}, {:?});", base_path, path)?;
            }
            writer.deep_copy_file_from_path(&*base_path, &*path)?;
            *files_added += 1;
        }
        BasicFileOperation::MergeWithOtherFile { operations } => {
            let mut other_writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            let mut inner_files_added = 0;
            if let Some(ref mut stringifier) = stringifier {
                writeln!(stringifier, "let sub_writer = {{")?;
            }
            operations.iter().for_each(|(operation, abort)| {
                let _ = do_operation(
                    &mut other_writer,
                    &operation,
                    *abort,
                    false,
                    &mut inner_files_added,
                    stringifier
                );
            });
            if let Some(ref mut stringifier) = stringifier {
                writeln!(stringifier, "}}\nwriter.merge_archive(sub_writer)?;")?;
            }
            writer.merge_archive(other_writer.finish_into_readable()?)?;
            *files_added += inner_files_added;
        }
        BasicFileOperation::SetArchiveComment(comment) => {
            if let Some(ref mut stringifier) = stringifier {
                writeln!(stringifier, "writer.set_raw_comment({:?})?;", comment)?;
            }
            writer.set_raw_comment(comment.clone());
        }
    }
    if abort && *files_added != 0 {
        if let Some(ref mut stringifier) = stringifier {
            writeln!(stringifier, "writer.abort_file()?;")?;
        }
        writer.abort_file()?;
        *files_added -= 1;
    }
    // If a comment is set, we finish the archive, reopen it for append and then set a shorter
    // comment, then there will be junk after the new comment that we can't get rid of. Thus, we
    // can only check that the expected is a prefix of the actual
    match operation.reopen {
        ReopenOption::DoNotReopen => {
            if let Some(ref mut stringifier) = stringifier {
                writeln!(stringifier, "writer")?;
            }
            return Ok(())
        },
        ReopenOption::ViaFinish => {
            let old_comment = writer.get_raw_comment().to_owned();
            writeln!(stringifier, "let writer = ZipWriter::new_append(writer.finish()?)?;")?;
            replace_with_or_abort(writer, |old_writer: zip::ZipWriter<T>| {
                zip::ZipWriter::new_append(old_writer.finish().unwrap()).unwrap()
            });
            assert!(writer.get_raw_comment().starts_with(&old_comment));
        }
        ReopenOption::ViaFinishIntoReadable => {
            let old_comment = writer.get_raw_comment().to_owned();
            writeln!(stringifier, "let writer = ZipWriter::new_append(writer.finish_into_readable()?)?;")?;
            replace_with_or_abort(writer, |old_writer: zip::ZipWriter<T>| {
                zip::ZipWriter::new_append(old_writer.finish_into_readable().unwrap().into_inner())
                    .unwrap()
            });
            assert!(writer.get_raw_comment().starts_with(&old_comment));
        }
    }
    Ok(())
}

impl <'k> FuzzTestCase<'k> {
    fn execute(&self, stringifier: &mut impl std::fmt::Write) -> ZipResult<()> {
        let mut files_added = 0;
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let mut final_reopen = false;
        if let Some((last_op, _)) = self.operations.last() {
            if last_op.reopen != ReopenOption::ViaFinishIntoReadable {
                final_reopen = true;
            }
        }
        #[allow(unknown_lints)]
        #[allow(boxed_slice_into_iter)]
        for (operation, abort) in self.operations.iter() {
            let _ = do_operation(
                &mut writer,
                &operation,
                *abort,
                self.flush_on_finish_file,
                &mut files_added,
                stringifier
            );
        }
        if final_reopen {
            writeln!(stringifier, "let _ = writer.finish_into_readable()?;")
                .map_err(|_| ZipError::InvalidArchive(""))?;
            let _ = writer.finish_into_readable()?;
        }
        Ok(())
    }
}

impl <'k> Debug for FuzzTestCase<'k> {
    fn fmt(&self, f: &mut impl std::fmt::Write) -> std::fmt::Result {
        Ok(self.execute(f).map_err(|e| {
            let _ = e.fmt(f);
            Default::default()
        })?)
    }
}

#[derive(Default)]
struct NoopWrite {}

impl std::fmt::Write for NoopWrite {
    fn write_str(&mut self, _: &str) -> std::fmt::Result {
        Ok(())
    }

    fn write_char(&mut self, _: char) -> std::fmt::Result {
        Ok(())
    }

    fn write_fmt(&mut self, _: Arguments<'_>) -> std::fmt::Result {
        Ok(())
    }
}

fuzz_target!(|test_case: FuzzTestCase| {
    test_case.execute(&mut NoopWrite::default()).unwrap()
});
