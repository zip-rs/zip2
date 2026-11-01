//! A copy under a new name must not keep a Unicode Path (0x7075) extra field that would
//! rename the copy when read back (found by fuzz_write: the copy collided with its source).

use std::io::{Cursor, Write};
use zip::write::{FullFileOptions, ZipWriter};
use zip::{CompressionMethod, ZipArchive};

fn source_archive() -> ZipWriter<Cursor<Vec<u8>>> {
    // The field's CRC matches "b.txt", not "a.txt", so readers ignore it on "a.txt", but
    // would apply it to a copy named "b.txt" and rename that copy to "a.txt".
    let mut data = vec![1u8];
    data.extend_from_slice(&crc32fast::hash(b"b.txt").to_le_bytes());
    data.extend_from_slice(b"a.txt");
    let mut options = FullFileOptions::default().compression_method(CompressionMethod::Stored);
    options.add_extra_field(0x7075, data, false).unwrap();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("a.txt", options).unwrap();
    writer.write_all(b"hello").unwrap();
    ZipWriter::new_append(writer.finish().unwrap()).unwrap()
}

fn assert_both_names(writer: ZipWriter<Cursor<Vec<u8>>>) {
    let archive = ZipArchive::new(writer.finish().unwrap()).unwrap();
    let mut names: Vec<String> = archive
        .file_names()
        .map(|name| name.unwrap().into_owned())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["a.txt", "b.txt"]);
}

#[test]
fn deep_copy_drops_stale_unicode_path() {
    let mut writer = source_archive();
    writer.deep_copy_file("a.txt", "b.txt").unwrap();
    assert_both_names(writer);
}

#[test]
fn shallow_copy_drops_stale_unicode_path() {
    let mut writer = source_archive();
    writer.shallow_copy_file("a.txt", "b.txt").unwrap();
    assert_both_names(writer);
}

use zip::ZipUpdate;
