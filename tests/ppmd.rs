#![cfg(feature = "ppmd")]

use std::io::{self, Read, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive};

const IPSUM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit.\n";

#[test]
fn decompress_ppmd() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/ppmd.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("could not open test zip file");

    let mut file = archive
        .by_name("ipsum.txt")
        .expect("could not find file in archive");
    assert_eq!("ipsum.txt", file.name());

    let mut data = Vec::new();
    file.read_to_end(&mut data).expect("could not read data");
    let content = String::from_utf8(data).expect("data is not valid UTF-8");

    assert_eq!(IPSUM, content);
}

#[test]
fn compress_ppmd() {
    let file_name: &str = "test.txt";

    let cursor = {
        let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        zip.start_file(
            file_name,
            SimpleFileOptions::default()
                .compression_method(CompressionMethod::Ppmd)
                .compression_level(Some(9)),
        )
        .unwrap();
        zip.write_all(IPSUM.as_bytes()).unwrap();

        zip.finish().unwrap()
    };

    let mut archive = ZipArchive::new(cursor).expect("could not open test zip file");

    let mut file = archive.by_name(file_name).expect("could not find file");

    let mut data = Vec::new();
    file.read_to_end(&mut data).expect("could not read data");
    let content = String::from_utf8(data).expect("data is not valid UTF-8");

    assert_eq!(IPSUM, content);
}
