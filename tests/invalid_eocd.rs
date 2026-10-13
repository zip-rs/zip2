//! Tests on EOCD

use std::io::{Cursor, Write};

use zip::unstable::format::magic::Magic;
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

#[test]
fn check_eocd_large_fields() {
    let mut data = Vec::new();
    let mut writer = ZipWriter::new(Cursor::new(&mut data));
    writer
        .start_file(
            "a",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(b"aaa").unwrap();
    writer.finish().unwrap();

    let eocd_start = data.len() - 22;
    assert_eq!(
        data[eocd_start..eocd_start + 4],
        Magic::CENTRAL_DIRECTORY_END_SIGNATURE.to_le_bytes()
    );
    //  size of the central directory
    data[eocd_start + 13] = 0xFF;
    data[eocd_start + 14] = 0xFF;
    data[eocd_start + 15] = 0xFF;
    data[eocd_start + 16] = 0xFE;

    let archive_res = ZipArchive::new(Cursor::new(data));
    assert!(archive_res.is_err()); // Size of central in doing the failure
}

#[test]
fn check_eocd_more_than_total() {
    let mut data = Vec::new();
    let mut writer = ZipWriter::new(Cursor::new(&mut data));
    writer
        .start_file(
            "a",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(b"aaa").unwrap();
    writer
        .start_file(
            "b",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(b"bbb").unwrap();
    writer.finish().unwrap();

    let eocd_start = data.len() - 22;
    assert_eq!(
        data[eocd_start..eocd_start + 4],
        Magic::CENTRAL_DIRECTORY_END_SIGNATURE.to_le_bytes()
    );
    assert_eq!(data[eocd_start..].len(), 22);
    // total number of entries in the central directory on this disk
    data[eocd_start + 8] = 0xFF;
    data[eocd_start + 9] = 0xFE;
    // total number of entries in the central directory
    data[eocd_start + 10] = 0xFF;
    data[eocd_start + 11] = 0xFE;
    eprintln!("{:x?}", data);

    let archive_res = ZipArchive::new(Cursor::new(data));
    assert!(archive_res.is_err()); // more entries, the archive try to parse a ZipCentralBlock
}

#[test]
fn check_eocd_less_than_total() {
    let mut data = Vec::new();
    let mut writer = ZipWriter::new(Cursor::new(&mut data));
    writer
        .start_file(
            "a",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(b"aaa").unwrap();
    writer
        .start_file(
            "b",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(b"bbb").unwrap();
    writer.finish().unwrap();

    let eocd_start = data.len() - 22;
    assert_eq!(
        data[eocd_start..eocd_start + 4],
        Magic::CENTRAL_DIRECTORY_END_SIGNATURE.to_le_bytes()
    );
    assert_eq!(data[eocd_start..].len(), 22);
    // total number of entries in the central directory on this disk
    data[eocd_start + 8] = 0x01;
    data[eocd_start + 9] = 0x00;
    // total number of entries in the central directory
    data[eocd_start + 10] = 0x01;
    data[eocd_start + 11] = 0x00;
    eprintln!("{:x?}", data);

    let mut archive = ZipArchive::new(Cursor::new(data)).unwrap();
    assert_eq!(archive.len(), 1);
    archive.by_name("a").unwrap();
}
