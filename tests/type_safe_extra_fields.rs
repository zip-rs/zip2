//! Integration test for type-safe extra fields constructors and FileOptions builders.

use std::io::{Cursor, Read, Seek, SeekFrom, Write};

use zip::extra_fields::{ExtendedTimestamp, ExtraField, Ntfs};
use zip::read::read_zipfile_from_stream;
use zip::write::{FullFileOptions, SimpleFileOptions, ZipFileBuilder, ZipWriter};
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

    // 3. Verify central directory extra fields in source archive (only flags + ModTime, 5-byte payload)
    let mut source_archive = ZipArchive::new(Cursor::new(source_bytes.clone())).unwrap();
    assert_central_fields(&mut source_archive);
    let source_cd_start = source_archive
        .by_index_raw(0)
        .unwrap()
        .central_header_start();
    let source_cd_ts = find_extra_field(central_extra_data(&source_bytes, source_cd_start), 0x5455)
        .expect("ExtendedTimestamp missing from source central directory");
    let mut expected_source_cd_ts = vec![0x07];
    expected_source_cd_ts.extend_from_slice(&MODIFIED_UNIX.to_le_bytes());
    assert_eq!(source_cd_ts, expected_source_cd_ts.as_slice());

    // 4. Perform raw copy to a second archive
    let mut copy_writer = ZipWriter::new(Cursor::new(Vec::new()));
    let source_file = source_archive.by_index_raw(0).unwrap();
    copy_writer.raw_copy_file(source_file).unwrap();
    let copied_bytes = copy_writer.finish().unwrap().into_inner();

    // 5. Verify copied archive preserves extra fields and contents
    let mut copied_archive = ZipArchive::new(Cursor::new(copied_bytes.clone())).unwrap();
    assert_central_fields(&mut copied_archive);
    let copied_cd_start = copied_archive
        .by_index_raw(0)
        .unwrap()
        .central_header_start();
    let copied_cd_ts = find_extra_field(central_extra_data(&copied_bytes, copied_cd_start), 0x5455)
        .expect("ExtendedTimestamp missing from copied central directory");
    let mut expected_copied_cd_ts = vec![0x01];
    expected_copied_cd_ts.extend_from_slice(&MODIFIED_UNIX.to_le_bytes());
    assert_eq!(copied_cd_ts, expected_copied_cd_ts.as_slice());

    let mut copied_file = copied_archive.by_name("entry.txt").unwrap();
    let mut contents = Vec::new();
    copied_file.read_to_end(&mut contents).unwrap();
    assert_eq!(contents, b"high-precision timestamps");
}

fn local_extra_data(bytes: &[u8], header_start: u64) -> &[u8] {
    let header = &bytes[header_start as usize..];
    let file_name_len = u16::from_le_bytes([header[26], header[27]]) as usize;
    let extra_len = u16::from_le_bytes([header[28], header[29]]) as usize;
    &header[30 + file_name_len..30 + file_name_len + extra_len]
}

fn central_extra_data(bytes: &[u8], central_header_start: u64) -> &[u8] {
    let header = &bytes[central_header_start as usize..];
    let file_name_len = u16::from_le_bytes([header[28], header[29]]) as usize;
    let extra_len = u16::from_le_bytes([header[30], header[31]]) as usize;
    &header[46 + file_name_len..46 + file_name_len + extra_len]
}

fn find_extra_field(extra_bytes: &[u8], target_id: u16) -> Option<&[u8]> {
    let mut pos = 0;
    while pos + 4 <= extra_bytes.len() {
        let header_id = u16::from_le_bytes([extra_bytes[pos], extra_bytes[pos + 1]]);
        let len = u16::from_le_bytes([extra_bytes[pos + 2], extra_bytes[pos + 3]]) as usize;
        let payload_start = pos + 4;
        let payload_end = payload_start.checked_add(len)?;
        let payload = extra_bytes.get(payload_start..payload_end)?;
        if header_id == target_id {
            return Some(payload);
        }
        pos = payload_end;
    }
    None
}

fn verify_local_header_extended_timestamp(bytes: &[u8]) {
    let payload = find_extra_field(local_extra_data(bytes, 0), 0x5455)
        .expect("ExtendedTimestamp not found in local header");
    assert_eq!(payload.len(), 13);
    let mut cursor = Cursor::new(payload);
    let ts = ExtendedTimestamp::try_from_reader(&mut cursor, payload.len() as u16, true).unwrap();
    assert_eq!(ts.mod_time(), Some(MODIFIED_UNIX));
    assert_eq!(ts.ac_time(), Some(ACCESSED_UNIX));
    assert_eq!(ts.cr_time(), Some(CREATED_UNIX));
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

#[test]
fn test_extended_timestamp_local_only_times_not_leaked_to_central_directory() {
    // Only accessed and created timestamps are provided (modified = None, flags = 0b110 = 6).
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .with_extended_timestamp(None, Some(ACCESSED_UNIX), Some(CREATED_UNIX))
        .unwrap();

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("local_only.txt", options).unwrap();
    writer.write_all(b"payload").unwrap();
    let archive_bytes = writer.finish().unwrap().into_inner();

    // Local header must carry flags = 6 + AcTime + CrTime (9-byte payload).
    let local_ts_bytes = find_extra_field(local_extra_data(&archive_bytes, 0), 0x5455)
        .expect("ExtendedTimestamp missing from local header");
    assert_eq!(local_ts_bytes.len(), 9);
    assert_eq!(local_ts_bytes[0], 0x06);
    assert_eq!(&local_ts_bytes[1..5], &ACCESSED_UNIX.to_le_bytes());
    assert_eq!(&local_ts_bytes[5..9], &CREATED_UNIX.to_le_bytes());

    // Central directory must carry only the 1-byte flags field (no AcTime/CrTime).
    let mut archive = ZipArchive::new(Cursor::new(&archive_bytes)).unwrap();
    let file = archive.by_name("local_only.txt").unwrap();
    let cd_ts_bytes = find_extra_field(
        central_extra_data(&archive_bytes, file.central_header_start()),
        0x5455,
    )
    .expect("ExtendedTimestamp missing from central directory");
    assert_eq!(cd_ts_bytes, &[0x06]);

    // Reader must not misinterpret AcTime as ModTime.
    let ts = file
        .extra_data_fields()
        .find_map(|field| match field {
            ExtraField::ExtendedTimestamp(ts) => Some(ts),
            _ => None,
        })
        .expect("ExtendedTimestamp extra field missing");
    assert_eq!(ts.mod_time(), None);
    assert_eq!(ts.ac_time(), None);
    assert_eq!(ts.cr_time(), None);
    drop(file);

    // new_append + deep_copy_file must keep original central flags = 6 and set copied flags = 0.
    let mut append_writer = ZipWriter::new_append(Cursor::new(archive_bytes.clone())).unwrap();
    append_writer
        .deep_copy_file("local_only.txt", "deep_copy.txt")
        .unwrap();
    let appended_bytes = append_writer.finish().unwrap().into_inner();
    let mut appended = ZipArchive::new(Cursor::new(&appended_bytes)).unwrap();

    for (name, expected_cd_flag, expected_local_flag) in [
        ("local_only.txt", 0x06u8, 0x06u8),
        ("deep_copy.txt", 0x00u8, 0x00u8),
    ] {
        let entry = appended.by_name(name).unwrap();
        let entry_cd_ts = find_extra_field(
            central_extra_data(&appended_bytes, entry.central_header_start()),
            0x5455,
        )
        .expect("ExtendedTimestamp missing in appended central directory");
        assert_eq!(entry_cd_ts, &[expected_cd_flag]);

        let mut stream = Cursor::new(&appended_bytes);
        stream.seek(SeekFrom::Start(entry.header_start())).unwrap();
        let local_entry = read_zipfile_from_stream(&mut stream).unwrap().unwrap();
        let local_ts = local_entry
            .extra_data_fields()
            .find_map(|field| match field {
                ExtraField::ExtendedTimestamp(ts) => Some(ts),
                _ => None,
            })
            .expect("ExtendedTimestamp missing in local stream");
        assert_eq!(local_ts.mod_time(), None);
        assert_eq!(
            local_ts.ac_time(),
            (expected_local_flag == 0x06).then_some(ACCESSED_UNIX)
        );
        assert_eq!(
            local_ts.cr_time(),
            (expected_local_flag == 0x06).then_some(CREATED_UNIX)
        );
    }

    // raw_copy_file must keep mod_time = None with flags = 0 in both headers.
    let mut raw_writer = ZipWriter::new(Cursor::new(Vec::new()));
    raw_writer
        .raw_copy_file(archive.by_index_raw(0).unwrap())
        .unwrap();
    let raw_bytes = raw_writer.finish().unwrap().into_inner();
    let mut raw_archive = ZipArchive::new(Cursor::new(&raw_bytes)).unwrap();
    let raw_entry = raw_archive.by_index(0).unwrap();
    assert_eq!(
        find_extra_field(
            central_extra_data(&raw_bytes, raw_entry.central_header_start()),
            0x5455
        ),
        Some(&[0x00][..])
    );
    assert_eq!(
        find_extra_field(
            local_extra_data(&raw_bytes, raw_entry.header_start()),
            0x5455
        ),
        Some(&[0x00][..])
    );
}

#[test]
fn test_extended_timestamp_all_flag_combinations_and_prepared_file() {
    for flags in 0u8..8 {
        let modified = (flags & 1 != 0).then_some(MODIFIED_UNIX);
        let accessed = (flags & 2 != 0).then_some(ACCESSED_UNIX);
        let created = (flags & 4 != 0).then_some(CREATED_UNIX);

        let options = FullFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .validate_extra_fields(true)
            .with_extended_timestamp(modified, accessed, created)
            .unwrap();

        let mut builder = ZipFileBuilder::new("prepared.txt", options).unwrap();
        builder.write_all(b"data").unwrap();
        let prepared = builder.finish().unwrap();

        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.add_prepared_file(prepared).unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let entry = archive.by_name("prepared.txt").unwrap();

        let local_ts_bytes =
            find_extra_field(local_extra_data(&bytes, entry.header_start()), 0x5455)
                .expect("local ExtendedTimestamp");
        assert_eq!(local_ts_bytes.len(), 1 + 4 * flags.count_ones() as usize);
        assert_eq!(local_ts_bytes[0], flags);

        let cd_ts_bytes = find_extra_field(
            central_extra_data(&bytes, entry.central_header_start()),
            0x5455,
        )
        .expect("central ExtendedTimestamp");
        assert_eq!(cd_ts_bytes.len(), 1 + 4 * usize::from(modified.is_some()));
        assert_eq!(cd_ts_bytes[0], flags);

        let ts = entry
            .extra_data_fields()
            .find_map(|field| match field {
                ExtraField::ExtendedTimestamp(ts) => Some(ts),
                _ => None,
            })
            .expect("parsed central ExtendedTimestamp");
        assert_eq!(ts.mod_time(), modified);
        assert_eq!(ts.ac_time(), None);
        assert_eq!(ts.cr_time(), None);
    }
}
