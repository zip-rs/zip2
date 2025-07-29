use std::io::{Cursor, Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{AesMode, ZipArchive, ZipWriter};

const DIRECTORY_NAME: &str = "test_directory";
const FILE_NAME: &str = "hello_world.txt";
const LOREM_IPSUM: &[u8] = b"Lorem ipsum dolor sit amet, consectetur adipiscing elit.";
const PASSWORD: &str = "helloworld";

#[test]
fn by_path() {
    let mut buf = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(&mut buf));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.add_directory(DIRECTORY_NAME, options).unwrap();
    zip.start_file(format!("{DIRECTORY_NAME}/{FILE_NAME}"), options)
        .unwrap();
    zip.write_all(LOREM_IPSUM).unwrap();
    zip.finish().unwrap();

    let mut archive = ZipArchive::new(Cursor::new(&mut buf)).unwrap();
    let path = Path::new(DIRECTORY_NAME).join(FILE_NAME);
    let mut file = archive.by_path(path).unwrap();
    let mut file_buf = Vec::new();
    file.read_to_end(&mut file_buf).unwrap();
    assert_eq!(LOREM_IPSUM, file_buf);
}

#[test]
fn by_path_decrypt() {
    let mut buf = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(&mut buf));
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .with_aes_encryption(AesMode::Aes128, PASSWORD);
    zip.add_directory(DIRECTORY_NAME, options).unwrap();
    zip.start_file(format!("{DIRECTORY_NAME}/{FILE_NAME}"), options)
        .unwrap();
    zip.write_all(LOREM_IPSUM).unwrap();
    zip.finish().unwrap();

    let mut archive = ZipArchive::new(Cursor::new(&mut buf)).unwrap();
    let path = Path::new(DIRECTORY_NAME).join(FILE_NAME);
    let mut file = archive.by_path_decrypt(path, PASSWORD.as_bytes()).unwrap();
    let mut file_buf = Vec::new();
    file.read_to_end(&mut file_buf).unwrap();
    assert_eq!(LOREM_IPSUM, file_buf);
}
