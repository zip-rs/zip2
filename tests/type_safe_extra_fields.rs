//! Integration test for type-safe extra fields constructors and FileOptions builders.

use std::io::{Cursor, Read, Write};

use zip::extra_fields::{ExtendedTimestamp, ExtraField, Ntfs};
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::{CompressionMethod, ZipArchive};

const MODIFIED_UNIX: u32 = 1_715_000_001;
const ACCESSED_UNIX: u32 = 1_715_000_002;
const CREATED_UNIX: u32 = 1_715_000_003;

const MTIME_NTFS: u64 = 133_000_000_000_000_000;
const ATIME_NTFS: u64 = 133_000_000_000_000_001;
const CTIME_NTFS: u64 = 133_000_000_000_000_002;

#[test]
fn test_type_safe_extra_fields_in_writer_and_raw_copy() {
    // 1. Build an archive using fluent type-safe builders with all timestamps provided
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .with_extended_timestamp(Some(MODIFIED_UNIX), Some(ACCESSED_UNIX), Some(CREATED_UNIX))
        .unwrap()
        .with_ntfs_times(MTIME_NTFS, ATIME_NTFS, CTIME_NTFS)
        .unwrap();

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("entry.txt", options).unwrap();
    writer.write_all(b"high-precision timestamps").unwrap();
    let source_bytes = writer.finish().unwrap().into_inner();

    // 2. Verify that the local header contains the full ExtendedTimestamp with access and creation times
    verify_local_header_extended_timestamp(&source_bytes);

    // 3. Verify central directory extra fields in source archive
    let mut source_archive = ZipArchive::new(Cursor::new(source_bytes.clone())).unwrap();
    assert_central_fields(&mut source_archive);

    // 4. Perform raw copy to a second archive
    let mut copy_writer = ZipWriter::new(Cursor::new(Vec::new()));
    let source_file = source_archive.by_index_raw(0).unwrap();
    copy_writer.raw_copy_file(source_file).unwrap();
    let copied_bytes = copy_writer.finish().unwrap().into_inner();

    // 5. Verify copied archive preserves extra fields and contents
    let mut copied_archive = ZipArchive::new(Cursor::new(copied_bytes)).unwrap();
    assert_central_fields(&mut copied_archive);

    let mut copied_file = copied_archive.by_name("entry.txt").unwrap();
    let mut contents = Vec::new();
    copied_file.read_to_end(&mut contents).unwrap();
    assert_eq!(contents, b"high-precision timestamps");
}

fn verify_local_header_extended_timestamp(bytes: &[u8]) {
    // Parse Local File Header:
    // 26..28: file name length
    // 28..30: extra field length
    let file_name_len = u16::from_le_bytes([bytes[26], bytes[27]]) as usize;
    let extra_len = u16::from_le_bytes([bytes[28], bytes[29]]) as usize;
    let extra_start = 30 + file_name_len;
    let extra_bytes = &bytes[extra_start..extra_start + extra_len];

    let mut cursor = Cursor::new(extra_bytes);
    let mut found = false;
    while (cursor.position() as usize) < extra_bytes.len() {
        let pos = cursor.position() as usize;
        let header_id = u16::from_le_bytes([extra_bytes[pos], extra_bytes[pos + 1]]);
        let len = u16::from_le_bytes([extra_bytes[pos + 2], extra_bytes[pos + 3]]);
        cursor.set_position((pos + 4) as u64);
        if header_id == 0x5455 {
            let ts = ExtendedTimestamp::try_from_reader(&mut cursor, len, true).unwrap();
            assert_eq!(ts.mod_time(), Some(MODIFIED_UNIX));
            assert_eq!(ts.ac_time(), Some(ACCESSED_UNIX));
            assert_eq!(ts.cr_time(), Some(CREATED_UNIX));
            found = true;
            break;
        } else {
            cursor.set_position(cursor.position() + len as u64);
        }
    }
    assert!(found, "ExtendedTimestamp not found in local header");
}

fn assert_central_fields(archive: &mut ZipArchive<Cursor<Vec<u8>>>) {
    let file = archive.by_index_raw(0).unwrap();
    let mut found_extended_timestamp = false;
    let mut found_ntfs = false;

    // Check central directory extra fields
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

    assert!(
        found_extended_timestamp,
        "ExtendedTimestamp extra field missing from central directory"
    );
    assert!(
        found_ntfs,
        "Ntfs extra field missing from central directory"
    );
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
