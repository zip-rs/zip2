#![cfg(feature = "lzma")]

use std::io::{self, Read};
use zip::ZipArchive;

#[test]
pub fn decompress_lzma() {
    let v = include_bytes!("data/lzma.zip");
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
