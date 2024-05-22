#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use replace_with::replace_with_or_abort;
use std::io::{Cursor, Read, Seek, Write};
use std::path::PathBuf;
use zip::result::ZipError;

#[derive(Arbitrary, Clone, Debug)]
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
        operations: Box<[(FileOperation<'k>, bool)]>
    }
}

#[derive(Arbitrary, Clone, Debug, Eq, PartialEq)]
pub enum ReopenOption {
    DoNotReopen,
    ViaFinish,
    ViaFinishIntoReadable
}

#[derive(Arbitrary, Clone, Debug)]
pub struct FileOperation<'k> {
    basic: BasicFileOperation<'k>,
    path: PathBuf,
    reopen: ReopenOption,
    // 'abort' flag is separate, to prevent trying to copy an aborted file
}

#[derive(Arbitrary, Clone, Debug)]
pub struct FuzzTestCase<'k> {
    comment: Box<[u8]>,
    operations: Box<[(FileOperation<'k>, bool)]>,
    flush_on_finish_file: bool,
}

fn do_operation<'k, T>(
    writer: &mut zip::ZipWriter<T>,
    operation: &FileOperation<'k>,
    abort: bool,
    flush_on_finish_file: bool,
) -> Result<(), Box<dyn std::error::Error>>
where
    T: Read + Write + Seek,
{
    writer.set_flush_on_finish_file(flush_on_finish_file);
    let path = &operation.path;
    match &operation.basic {
        BasicFileOperation::WriteNormalFile {
            contents,
            options,
            ..
        } => {
            let uncompressed_size = contents.iter().map(|chunk| chunk.len()).sum::<usize>();
            let mut options = (*options).to_owned();
            if uncompressed_size >= u32::MAX as usize {
                options = options.large_file(true);
            }
            writer.start_file_from_path(path, options)?;
            for chunk in contents.iter() {
                writer.write_all(&chunk)?;
            }
        }
        BasicFileOperation::WriteDirectory(options) => {
            writer.add_directory_from_path(path, options.to_owned())?;
        }
        BasicFileOperation::WriteSymlinkWithTarget { target, options } => {
            writer.add_symlink_from_path(&path, target, options.to_owned())?;
        }
        BasicFileOperation::ShallowCopy(base) => {
            do_operation(writer, &base, false, flush_on_finish_file)?;
            writer.shallow_copy_file_from_path(&base.path, &path)?;
        }
        BasicFileOperation::DeepCopy(base) => {
            do_operation(writer, &base, false, flush_on_finish_file)?;
            writer.deep_copy_file_from_path(&base.path, &path)?;
        }
        BasicFileOperation::MergeWithOtherFile { operations } => {
            let mut other_writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
            operations.iter().for_each(|(operation, abort)| {
                let _ = do_operation(
                    &mut other_writer,
                    &operation,
                    *abort,
                    false,
                );
            });
            writer.merge_archive(other_writer.finish_into_readable()?)?;
        }
    }
    if abort {
        match writer.abort_file() {
            Ok(()) => {},
            Err(ZipError::FileNotFound) => {},
            Err(e) => return Err(Box::new(e))
        }
    }
    let old_comment = writer.get_raw_comment().to_owned();
    match operation.reopen {
        ReopenOption::DoNotReopen => {},
        ReopenOption::ViaFinish => replace_with_or_abort(writer, |old_writer: zip::ZipWriter<T>| {
            zip::ZipWriter::new_append(old_writer.finish().unwrap()).unwrap()
        }),
        ReopenOption::ViaFinishIntoReadable => replace_with_or_abort(writer, |old_writer: zip::ZipWriter<T>| {
            zip::ZipWriter::new_append(old_writer.finish_into_readable().unwrap().into_inner()).unwrap()
        }),
    }
    assert_eq!(&old_comment, writer.get_raw_comment());
    Ok(())
}

fuzz_target!(|test_case: FuzzTestCase| {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer.set_raw_comment(test_case.comment);
    let mut final_reopen = false;
    if let Some((last_op, _)) = test_case.operations.last() {
        if last_op.reopen != ReopenOption::ViaFinishIntoReadable {
            final_reopen = true;
        }
    }
    for (operation, abort) in test_case.operations.into_iter() {
        let _ = do_operation(
            &mut writer,
            &operation,
            *abort,
            test_case.flush_on_finish_file,
        );
    }
    if final_reopen {
        let _ = writer.finish_into_readable().unwrap();
    }
});
