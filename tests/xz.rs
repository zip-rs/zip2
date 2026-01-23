#![cfg(feature = "xz")]

use std::io::{self, Read};
use zip::ZipArchive;

#[test]
pub fn decompress_xz() {
    let v = include_bytes!("data/xz.zip");
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    let mut file = archive.by_name("hello.txt").expect("couldn't find hello.txt in archive");
    assert_eq!("hello.txt", file.name());

    let mut content = Vec::new();
    file.read_to_end(&mut content).unwrap();
    assert_eq!(
        "Hello world\n",
        String::from_utf8(content).expect("content from hello.txt should be valid UTF-8")
    );
}
