//! Test for the crc32
use std::io::Read;

fn generate_zip_with_wrong_crc32() -> Vec<u8> {
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    let mut data = Vec::new();
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(&mut data));
    archive.start_file("test.txt", options).unwrap();
    let fake_file = [0u8; 16];
    let mut f = std::io::Cursor::new(fake_file);
    std::io::copy(&mut f, &mut archive).unwrap();
    archive.finish().unwrap();

    println!("{data:#04x?}");
    // local header crc32
    data[14..18].copy_from_slice(&[0x01, 0x02, 0x03, 0x04]);
    // central header crc32
    data[70..74].copy_from_slice(&[0x05, 0x06, 0x07, 0x08]);
    println!("{data:#04x?}");

    data
}

#[test]
fn invalid_crc32_should_error() {
    let archive = generate_zip_with_wrong_crc32();
    let cursor = std::io::Cursor::new(archive);
    let mut reader = zip::ZipArchive::new(cursor).unwrap();

    let mut file = reader.by_name("test.txt").unwrap();
    let mut buf = Vec::new();
    let read_res = file.read_to_end(&mut buf);
    assert!(read_res.is_err());
    let err = read_res.unwrap_err();
    println!("{err:?}");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(err.to_string(), "Invalid checksum");
}

#[test]
fn invalid_crc32_ignored_should_success() {
    let archive = generate_zip_with_wrong_crc32();
    let cursor = std::io::Cursor::new(archive);
    let mut reader = zip::ZipArchive::new(cursor).unwrap();

    let read_options = zip::read::ZipReadOptions::new().ignore_crc32(true);
    let mut file = reader.by_index_with_options(0, read_options).unwrap();
    let mut buf = Vec::new();
    let read_res = file.read_to_end(&mut buf);
    assert!(read_res.is_ok());
}
