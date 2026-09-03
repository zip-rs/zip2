use std::io::{Cursor, Write};

// https://github.com/zip-rs/zip2/issues/971
#[test]
fn directory_should_not_have_a_data_descriptor() {
    use zip::{HasZipMetadata, ZipArchive, ZipWriter, write::SimpleFileOptions};
    const LOCAL_FILE_HEADER_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
    let mut writer = ZipWriter::new_stream(Vec::new());
    writer
        .add_directory("mydir", SimpleFileOptions::default())
        .unwrap();
    writer
        .start_file("mydir/file.txt", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"hello").unwrap();
    let bytes = writer.finish().unwrap().into_inner();

    let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let directory = archive.by_name("mydir/").unwrap();
    let metadata = directory.get_metadata();
    let data_start = directory.data_start().unwrap() as usize;

    println!(
        "dir: using_data_descriptor={}",
        metadata.flags.is_using_data_descriptor()
    );
    assert_eq!(metadata.flags.is_using_data_descriptor(), false);

    let next_bytes: [u8; 4] = bytes[data_start..data_start + 4].try_into().unwrap();
    println!("bytes right after dir's data: {next_bytes:02x?}");

    assert_eq!(next_bytes, LOCAL_FILE_HEADER_SIGNATURE,);
}
