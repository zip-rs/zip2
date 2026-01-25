#![cfg(feature = "xz")]

use std::io::{self, Read};
use zip::ZipArchive;

#[test]
fn decompress_xz() {
    let zip_data = include_bytes!("data/xz.zip");
    let mut archive =
        ZipArchive::new(io::Cursor::new(zip_data)).expect("couldn't open test zip file");

    let mut file = archive
        .by_name("hello.txt")
        .expect("couldn't find hello.txt in archive");
    assert_eq!("hello.txt", file.name());

    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .expect("failed to read content from hello.txt");
    assert_eq!(
        "Hello world\n",
        String::from_utf8(content).expect("content from hello.txt should be valid UTF-8")
    );
}
