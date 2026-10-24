use std::io::{Cursor, Write};
use zip::result::ZipResult;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

#[test]
fn test_zip_writer_new_stream_with_comment() -> ZipResult<()> {
    let mut output = Vec::new();
    let mut writer = ZipWriter::new_stream(&mut output);

    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.start_file("test.txt", opts)?;
    writer.write_all(b"hello world")?;
    writer.set_comment("archive comment")?;

    writer.finish()?;
    Ok(())
}

#[test]
fn test_zip_writer_cursor_position_with_comment() -> ZipResult<()> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));

    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.start_file("test.txt", opts)?;
    writer.write_all(b"hello world")?;
    writer.set_comment("archive comment")?;

    let cursor = writer.finish()?;
    let total_len = cursor.get_ref().len() as u64;
    let pos = cursor.position();
    assert_eq!(
        pos, total_len,
        "Cursor position ({pos}) should be at the end of the archive ({total_len})"
    );
    Ok(())
}
