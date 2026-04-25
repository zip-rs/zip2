#![cfg(feature = "deflate64")]

use std::io::{self, Read};
use zip::ZipArchive;

#[test]
pub fn decompress_deflate64() {
    let v = include_bytes!("data/deflate64.zip");
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    let mut file = archive
        .by_name("binary.wmv")
        .expect("couldn't find file in archive");
    let file_name = file.name().unwrap();
    assert_eq!("binary.wmv", file_name);

    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .expect("couldn't read encrypted and compressed file");
    assert_eq!(include_bytes!("data/folder/binary.wmv"), &content[..]);
}
