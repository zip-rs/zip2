//! Integration test for type-safe extra fields constructors and FileOptions builders.

use std::io::{Cursor, Read, Write};

use zip::extra_fields::{ExtendedTimestamp, ExtraField, Ntfs};
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::{CompressionMethod, ZipArchive};

const MODIFIED_UNIX: u32 = 1_715_000_001;
const MTIME_NTFS: u64 = 133_000_000_000_000_000;
const ATIME_NTFS: u64 = 133_000_000_000_000_001;
const CTIME_NTFS: u64 = 133_000_000_000_000_002;

#[test]
fn test_type_safe_extra_fields_in_writer_and_raw_copy() {
    // 1. Build an archive using fluent type-safe builders starting from SimpleFileOptions
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .with_extended_timestamp(Some(MODIFIED_UNIX), None, None)
        .unwrap()
        .with_ntfs_times(MTIME_NTFS, ATIME_NTFS, CTIME_NTFS)
        .unwrap();

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("entry.txt", options).unwrap();
    writer.write_all(b"high-precision timestamps").unwrap();
    let source_bytes = writer.finish().unwrap().into_inner();

    // 2. Verify source archive extra fields
    let mut source_archive = ZipArchive::new(Cursor::new(source_bytes.clone())).unwrap();
    assert_archive_fields(&mut source_archive);

    // 3. Perform raw copy to a second archive
    let mut copy_writer = ZipWriter::new(Cursor::new(Vec::new()));
    let source_file = source_archive.by_index_raw(0).unwrap();
    copy_writer.raw_copy_file(source_file).unwrap();
    let copied_bytes = copy_writer.finish().unwrap().into_inner();

    // 4. Verify copied archive preserves extra fields and contents
    let mut copied_archive = ZipArchive::new(Cursor::new(copied_bytes)).unwrap();
    assert_archive_fields(&mut copied_archive);

    let mut copied_file = copied_archive.by_name("entry.txt").unwrap();
    let mut contents = Vec::new();
    copied_file.read_to_end(&mut contents).unwrap();
    assert_eq!(contents, b"high-precision timestamps");
}

fn assert_archive_fields(archive: &mut ZipArchive<Cursor<Vec<u8>>>) {
    let file = archive.by_index_raw(0).unwrap();
    let mut found_extended_timestamp = false;
    let mut found_ntfs = false;

    for field in file.extra_data_fields() {
        match field {
            ExtraField::ExtendedTimestamp(ts) => {
                found_extended_timestamp = true;
                assert_eq!(ts.mod_time(), Some(MODIFIED_UNIX));
            }
            ExtraField::Ntfs(ntfs) => {
                found_ntfs = true;
                assert_eq!(ntfs.mtime(), MTIME_NTFS);
                assert_eq!(ntfs.atime(), ATIME_NTFS);
                assert_eq!(ntfs.ctime(), CTIME_NTFS);
            }
            _ => {}
        }
    }

    assert!(found_extended_timestamp, "ExtendedTimestamp extra field missing");
    assert!(found_ntfs, "Ntfs extra field missing");
}

#[test]
fn test_direct_struct_constructors() {
    let ts = ExtendedTimestamp::new(Some(100), Some(200), Some(300));
    assert_eq!(ts.mod_time(), Some(100));
    assert_eq!(ts.ac_time(), Some(200));
    assert_eq!(ts.cr_time(), Some(300));

    let ntfs = Ntfs::new(1000, 2000, 3000);
    assert_eq!(ntfs.mtime(), 1000);
    assert_eq!(ntfs.atime(), 2000);
    assert_eq!(ntfs.ctime(), 3000);
}
