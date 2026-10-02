#![cfg(feature = "legacy-zip")]

use std::io::{self, Read};
use zip::ZipArchive;

#[test]
fn decompress_shrink() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/legacy/shrink.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    let mut file = archive
        .by_name("FIRST.TXT")
        .expect("couldn't find file in archive");
    let file_name = file.name().unwrap();
    assert_eq!("FIRST.TXT", file_name);

    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .expect("couldn't read encrypted and compressed file");
    assert_eq!(include_bytes!("data/folder/first.txt"), &content[..]);
}

/// A shrunk entry long enough for the LZW table to use every code up to MAX_CODE (8191) and then
/// do a partial clear. Made with hwzip 2.4 (`hwzip create -m shrink`), whose shrink is the
/// reference this decoder was ported from; Info-ZIP `unzip` extracts it to the same text.
/// The content is the first 26001 bytes of `seq 1 60000` (the numbers 1, 2, ... one per line):
/// the smallest prefix whose stream needs code 8191.
#[test]
fn decompress_shrink_full_code_table() {
    let mut archive = ZipArchive::new(io::Cursor::new(
        include_bytes!("data/legacy/shrink_full_table.zip").to_vec(),
    ))
    .expect("couldn't open test zip file");
    let mut file = archive.by_index(0).expect("couldn't find file in archive");

    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .expect("couldn't read shrunk file");

    let mut want = String::new();
    for n in 1..=60000 {
        want.push_str(&format!("{n}\n"));
    }
    assert_eq!(content.len(), 26001);
    assert_eq!(&content[..], &want.as_bytes()[..26001]);
}

#[test]
fn decompress_reduce() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/legacy/reduce.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    let mut file = archive
        .by_name("first.txt")
        .expect("couldn't find file in archive");
    let file_name = file.name().unwrap();
    assert_eq!("first.txt", file_name);

    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .expect("couldn't read encrypted and compressed file");
    assert_eq!(include_bytes!("data/folder/first.txt"), &content[..]);
}

#[test]
fn decompress_implode() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/legacy/implode.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    let mut file = archive
        .by_name("first.txt")
        .expect("couldn't find file in archive");
    let file_name = file.name().unwrap();
    assert_eq!("first.txt", file_name);

    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .expect("couldn't read encrypted and compressed file");
    assert_eq!(include_bytes!("data/folder/first.txt"), &content[..]);
}
