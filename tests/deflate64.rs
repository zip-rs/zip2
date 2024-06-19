#![cfg(feature = "deflate64")]

use std::io::{self, Read, Seek, SeekFrom};
use zip::ZipArchive;

#[test]
fn decompress_deflate64() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/deflate64.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    let mut file = archive
        .by_name("binary.wmv")
        .expect("couldn't find file in archive");
    assert_eq!("binary.wmv", file.name());

    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .expect("couldn't read encrypted and compressed file");
    assert_eq!(include_bytes!("data/folder/binary.wmv"), &content[..]);
}

#[test]
fn decompress_zip_file_entry_from_stream_error() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/data_descriptor.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(&v)).expect("couldn't open test zip file");

    // sanity check
    let file = archive
        .by_name("hello.txt")
        .expect("couldn't find file in archive");
    assert_eq!("hello.txt", file.name());

    // try to read zip file entry from stream
    let file_header_start = file.header_start();
    let mut stream = io::Cursor::new(&v);
    stream.seek(SeekFrom::Start(file_header_start)).expect("could not seek to file start");

    match zip::read::read_zipfile_from_stream(&mut stream) {
        Ok(_) => assert!(false, "There is no size data defined in the data descriptor"),
        Err(_) => {},
    };
}

#[test]
fn decompress_zip_file_entry_from_stream_success() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/data_descriptor_with_local_size.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(&v)).expect("couldn't open test zip file");

    // sanity check
    let file = archive
        .by_name("binary.wmv")
        .expect("couldn't find file in archive");
    assert_eq!("binary.wmv", file.name());

    // try to read zip file entry from stream
    let file_header_start = file.header_start();
    let mut stream = io::Cursor::new(&v);
    stream.seek(SeekFrom::Start(file_header_start)).expect("could not seek to file start");

    let mut file = zip::read::read_zipfile_from_stream(&mut stream)
        .expect("could not read zip file entry")
        .expect("could not read zip file entry");
    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .expect("couldn't read encrypted and compressed file");
    assert_eq!(include_bytes!("data/folder/binary.wmv"), &content[..]);
}
