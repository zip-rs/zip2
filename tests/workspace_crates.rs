use std::io::{Cursor, Write};

use zip::{
    add_file, extract, reader, update,
    write::{self, SimpleFileOptions},
};

#[test]
fn focused_crate_exports_share_types() {
    let output = Cursor::new(Vec::new());
    let mut archive = write::ZipWriter::new(output);
    archive
        .start_file("entry.txt", SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"workspace").unwrap();
    let output = archive.finish().unwrap();

    let mut archive = reader::ZipArchive::new(output).unwrap();
    let mut entry = archive.by_name("entry.txt").unwrap();
    let mut data = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
    assert_eq!(data, b"workspace");

    let _: Option<extract::ZipArchive<Cursor<Vec<u8>>>> = None;
    let _: Option<add_file::ZipWriter<Cursor<Vec<u8>>>> = None;
    let _: Option<update::ZipWriter<Cursor<Vec<u8>>>> = None;
}

#[test]
fn update_crate_exposes_append_and_readable_finish() {
    let output = write::ZipWriter::new(Cursor::new(Vec::new()))
        .finish()
        .unwrap();
    let mut writer =
        <update::ZipWriter<Cursor<Vec<u8>>> as update::ZipUpdate>::new_append(output).unwrap();
    writer
        .start_file("updated.txt", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"updated").unwrap();
    let archive =
        <update::ZipWriter<Cursor<Vec<u8>>> as update::ZipUpdate>::finish_into_readable(writer)
            .unwrap();
    assert_eq!(archive.len(), 1);
}
