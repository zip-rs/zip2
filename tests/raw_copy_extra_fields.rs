//! A raw copy has to keep the extra fields that the reader parsed for the source entry.
//!
//! `ZipWriter::raw_copy_file` passes them through `ExtraField::write`, which used to have no
//! arm for the extended timestamp (0x5455) and the NTFS file times (0x000a): the serialized
//! field came back empty and the copy silently lost it (see #1285).

use std::io::{Cursor, Write};

use zip::extra_fields::ExtraField;
use zip::write::{FullFileOptions, ZipWriter};
use zip::{CompressionMethod, ZipArchive};

const MODIFIED: u32 = 1_715_000_001;
const MTIME: u64 = 133_000_000_000_000_000;
const ATIME: u64 = 133_000_000_000_000_001;
const CTIME: u64 = 133_000_000_000_000_002;

/// Extended timestamp (0x5455) with only the modification time, as written in
/// the central directory.
fn extended_timestamp() -> Vec<u8> {
    let mut data = vec![0x01];
    data.extend_from_slice(&MODIFIED.to_le_bytes());
    data
}

/// NTFS file times (0x000a): reserved field, file-times attribute tag and
/// size, then the three Windows file times.
fn ntfs_times() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&0u32.to_le_bytes()); // reserved for future use
    data.extend_from_slice(&0x0001u16.to_le_bytes()); // attribute tag: file times
    data.extend_from_slice(&24u16.to_le_bytes()); // attribute size
    data.extend_from_slice(&MTIME.to_le_bytes());
    data.extend_from_slice(&ATIME.to_le_bytes());
    data.extend_from_slice(&CTIME.to_le_bytes());
    data
}

fn source_archive() -> Vec<u8> {
    let mut options = FullFileOptions::default().compression_method(CompressionMethod::Stored);
    options
        .add_extra_field(0x5455, extended_timestamp(), false)
        .unwrap();
    options
        .add_extra_field(0x000a, ntfs_times(), false)
        .unwrap();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("a.txt", options).unwrap();
    writer.write_all(b"hello").unwrap();
    writer.finish().unwrap().into_inner()
}

fn assert_fields(archive: &mut ZipArchive<Cursor<Vec<u8>>>, expected: bool) {
    let file = archive.by_index_raw(0).unwrap();
    let mut found_timestamp = false;
    let mut found_ntfs = false;
    for field in file.extra_data_fields() {
        match field {
            ExtraField::ExtendedTimestamp(timestamp) => {
                found_timestamp = true;
                assert_eq!(timestamp.mod_time(), Some(MODIFIED));
            }
            ExtraField::Ntfs(ntfs) => {
                found_ntfs = true;
                assert_eq!(ntfs.mtime(), MTIME);
                assert_eq!(ntfs.atime(), ATIME);
                assert_eq!(ntfs.ctime(), CTIME);
            }
            _ => {}
        }
    }
    assert_eq!(found_timestamp, expected, "extended timestamp field");
    assert_eq!(found_ntfs, expected, "NTFS file times field");
}

#[test]
fn raw_copy_keeps_the_extended_timestamp_and_ntfs_times() {
    let source = source_archive();

    // The source archive has both fields, so the copy has something to keep.
    let mut archive = ZipArchive::new(Cursor::new(source.clone())).unwrap();
    assert_fields(&mut archive, true);

    let mut copy = ZipWriter::new(Cursor::new(Vec::new()));
    copy.raw_copy_file(archive.by_index_raw(0).unwrap())
        .unwrap();
    let copied = copy.finish().unwrap().into_inner();

    let mut copy_archive = ZipArchive::new(Cursor::new(copied)).unwrap();
    assert_fields(&mut copy_archive, true);

    // The copied entry is still readable, and its contents are intact.
    let mut file = copy_archive.by_name("a.txt").unwrap();
    let mut contents = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut contents).unwrap();
    assert_eq!(contents, b"hello");
}
