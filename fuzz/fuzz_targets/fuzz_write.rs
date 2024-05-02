#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use replace_with::replace_with_or_abort;
use std::io::{Cursor, Read, Seek, Write};
use std::path::PathBuf;

#[derive(Arbitrary, Clone, Debug)]
pub enum BasicFileOperation<'k> {
    WriteNormalFile {
        contents: Vec<Vec<u8>>,
        options: zip::write::FullFileOptions<'k>,
    },
    WriteDirectory(zip::write::FullFileOptions<'k>),
    WriteSymlinkWithTarget {
        target: PathBuf,
        options: zip::write::FullFileOptions<'k>,
    },
    ShallowCopy(Box<FileOperation<'k>>),
    DeepCopy(Box<FileOperation<'k>>),
}

#[derive(Arbitrary, Clone, Debug)]
pub struct FileOperation<'k> {
    basic: BasicFileOperation<'k>,
    path: PathBuf,
    reopen: bool,
    // 'abort' flag is separate, to prevent trying to copy an aborted file
}

#[derive(Arbitrary, Clone, Debug)]
pub struct FuzzTestCase<'k> {
    comment: Vec<u8>,
    operations: Vec<(FileOperation<'k>, bool)>,
    flush_on_finish_file: bool,
}

fn do_operation<T>(
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
            let uncompressed_size = contents.iter().map(Vec::len).sum::<usize>();
            let mut options = (*options).to_owned();
            if uncompressed_size >= u32::MAX as usize {
                options = options.large_file(true);
            }
            writer.start_file_from_path(path, options)?;
            for chunk in contents {
                writer.write_all(chunk.as_slice())?;
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
    }
    if abort {
        writer.abort_file().unwrap();
    }
    if operation.reopen {
        let old_comment = writer.get_raw_comment().to_owned();
        replace_with_or_abort(writer, |old_writer: zip::ZipWriter<T>| {
            let new_writer =
                zip::ZipWriter::new_append(old_writer.finish().unwrap()).unwrap();
            assert_eq!(&old_comment, new_writer.get_raw_comment());
            new_writer
        });
    }
    Ok(())
}

fuzz_target!(|test_case: FuzzTestCase| {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer.set_raw_comment(test_case.comment);
    for (operation, abort) in test_case.operations {
        let _ = do_operation(
            &mut writer,
            &operation,
            abort,
            test_case.flush_on_finish_file,
        );
    }
    let _ = zip::ZipArchive::new(writer.finish().unwrap());
});
