//! Tests on extended timestamp

use zip::{ZipArchive, read::read_zipfile_from_stream};

#[test]
fn raw_copy_preserves_extended_timestamp() {
    use std::io::{Cursor, Read, Write};
    use zip::{CompressionMethod, ExtraField, ZipWriter, write::FullFileOptions};

    for modified in [Some(1_715_000_001u32), None] {
        let mut timestamp = vec![u8::from(modified.is_some())];
        if let Some(modified) = modified {
            timestamp.extend_from_slice(&modified.to_le_bytes());
        }
        let mut options = FullFileOptions::default().compression_method(CompressionMethod::Stored);
        options.add_extra_field(0x5455, &timestamp, false).unwrap();
        let mut source = ZipWriter::new(Cursor::new(Vec::new()));
        source.start_file("test.txt", options).unwrap();
        source.write_all(b"hello").unwrap();
        let mut source = ZipArchive::new(source.finish().unwrap()).unwrap();

        for rename in [false, true] {
            let mut copy = ZipWriter::new(Cursor::new(Vec::new()));
            let file = source.by_index_raw(0).unwrap();
            if rename {
                copy.raw_copy_file_rename(file, "renamed.txt").unwrap();
            } else {
                copy.raw_copy_file(file).unwrap();
            }
            let copied = copy.finish().unwrap().into_inner();
            let mut archive = ZipArchive::new(Cursor::new(&copied)).unwrap();
            let mut file = archive.by_index(0).unwrap();
            let timestamp = file.extra_data_fields().find_map(|field| match field {
                ExtraField::ExtendedTimestamp(timestamp) => Some(timestamp),
                _ => None,
            });
            assert_eq!(
                timestamp.expect("copied extended timestamp").mod_time(),
                modified
            );
            assert_eq!(
                file.name().unwrap(),
                if rename { "renamed.txt" } else { "test.txt" }
            );
            let mut contents = String::new();
            file.read_to_string(&mut contents).unwrap();
            assert_eq!(contents, "hello");

            let mut stream = Cursor::new(&copied);
            let file = read_zipfile_from_stream(&mut stream).unwrap().unwrap();
            let timestamp = file.extra_data_fields().find_map(|field| match field {
                ExtraField::ExtendedTimestamp(timestamp) => Some(timestamp),
                _ => None,
            });
            assert_eq!(
                timestamp.expect("local extended timestamp").mod_time(),
                modified
            );
        }
    }
}

#[test]
fn raw_copy_stream_timestamp_flags_match_local_contents() {
    use std::io::Cursor;
    use zip::{CompressionMethod, ExtraField, ZipWriter, write::FullFileOptions};

    // Access and creation times are local-only; raw_copy_file copies the central form.
    for flags in [2u8, 4, 6, 7] {
        let mut timestamp = vec![flags];
        for bit in [1, 2, 4] {
            if flags & bit != 0 {
                timestamp.extend_from_slice(&1_715_000_001u32.to_le_bytes());
            }
        }
        let mut options = FullFileOptions::default().compression_method(CompressionMethod::Stored);
        options.add_extra_field(0x5455, timestamp, false).unwrap();
        let mut source = ZipWriter::new(Cursor::new(Vec::new()));
        source.start_file("test.txt", options).unwrap();
        let mut source = Cursor::new(source.finish().unwrap().into_inner());
        let file = read_zipfile_from_stream(&mut source).unwrap().unwrap();
        let mut copy = ZipWriter::new(Cursor::new(Vec::new()));
        copy.raw_copy_file(file).unwrap();
        let mut copied = Cursor::new(copy.finish().unwrap().into_inner());
        let file = read_zipfile_from_stream(&mut copied).unwrap().unwrap();
        let timestamp = file.extra_data_fields().find_map(|field| match field {
            ExtraField::ExtendedTimestamp(timestamp) => Some(timestamp),
            _ => None,
        });
        let timestamp = timestamp.expect("readable local extended timestamp");
        assert_eq!(
            timestamp.mod_time(),
            (flags & 1 != 0).then_some(1_715_000_001)
        );
        assert_eq!(timestamp.ac_time(), None);
        assert_eq!(timestamp.cr_time(), None);
    }
}

#[test]
fn raw_copy_touch_does_not_preserve_old_extended_timestamp() {
    use std::io::Cursor;
    use zip::{CompressionMethod, DateTime, ExtraField, ZipWriter, write::FullFileOptions};

    let mut options = FullFileOptions::default().compression_method(CompressionMethod::Stored);
    options
        .add_extra_field(0x5455, [1, 1, 2, 3, 4], false)
        .unwrap();
    let mut source = ZipWriter::new(Cursor::new(Vec::new()));
    source.start_file("test.txt", options).unwrap();
    let mut source = ZipArchive::new(source.finish().unwrap()).unwrap();
    let modified = DateTime::from_date_and_time(2024, 1, 2, 3, 4, 6).unwrap();
    let mut copy = ZipWriter::new(Cursor::new(Vec::new()));
    copy.raw_copy_file_touch(source.by_index_raw(0).unwrap(), modified, None)
        .unwrap();
    let mut copied = ZipArchive::new(copy.finish().unwrap()).unwrap();
    let file = copied.by_index(0).unwrap();
    assert_eq!(file.last_modified(), Some(modified));
    assert!(
        !file
            .extra_data_fields()
            .any(|field| matches!(field, ExtraField::ExtendedTimestamp(_)))
    );
}

#[test]
fn append_preserves_extended_timestamp() {
    use std::io::Cursor;
    use zip::{CompressionMethod, ExtraField, ZipWriter, write::SimpleFileOptions};

    let source = Cursor::new(include_bytes!("data/extended_timestamp.zip").to_vec());
    let mut writer = ZipWriter::new_append(source).unwrap();
    writer
        .start_file(
            "new.txt",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
    let mut archive = ZipArchive::new(writer.finish().unwrap()).unwrap();
    let file = archive.by_name("test.txt").unwrap();
    let timestamp = file.extra_data_fields().find_map(|field| match field {
        ExtraField::ExtendedTimestamp(timestamp) => Some(timestamp),
        _ => None,
    });
    assert_eq!(
        timestamp.expect("appended extended timestamp").mod_time(),
        Some(1_714_635_025)
    );
    drop(file);
    assert!(archive.by_name("new.txt").is_ok());
}

#[test]
fn test_extended_timestamp() {
    use std::io::Cursor;

    let v = include_bytes!("../tests/data/extended_timestamp.zip");
    let mut archive = ZipArchive::new(Cursor::new(v)).expect("couldn't open test zip file");

    let mut extended_timestamp_field_found = false;
    for field in archive.by_name("test.txt").unwrap().extra_data_fields() {
        if let zip::ExtraField::ExtendedTimestamp(ts) = field {
            extended_timestamp_field_found = true;
            assert_eq!(ts.mod_time().unwrap(), 1714635025);
            assert!(ts.ac_time().is_none());
            assert!(ts.cr_time().is_none());
        }
    }
    assert!(extended_timestamp_field_found);
}

#[test]
fn test_extended_timestamp_stream() {
    use std::io::Cursor;

    let v = include_bytes!("../tests/data/extended_timestamp.zip");
    let mut stream = Cursor::new(v);

    // skip the first file
    read_zipfile_from_stream(&mut stream).unwrap().unwrap();
    let file = read_zipfile_from_stream(&mut stream).unwrap().unwrap();
    assert_eq!(file.name_raw(), "test.txt".as_bytes());

    let mut extended_timestamp_field_found = false;
    for field in file.extra_data_fields() {
        if let zip::ExtraField::ExtendedTimestamp(ts) = field {
            extended_timestamp_field_found = true;
            assert_eq!(ts.mod_time(), Some(1714635025));
            assert_eq!(ts.ac_time(), Some(1714635039));
            assert_eq!(ts.cr_time(), None);
        }
    }
    assert!(extended_timestamp_field_found);
}

#[test]
fn test_extended_timestamp_empty_central() {
    use std::io::Cursor;

    let zip = [
        0x50, 0x4B, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0x86,
        0xA6, 0x10, 0x36, 0x05, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x09, 0x00, 0x09, 0x00,
        0x68, 0x65, 0x6C, 0x6C, 0x6F, 0x2E, 0x74, 0x78, 0x74, 0x55, 0x54, 0x05, 0x00, 0x04, 0x00,
        0xF1, 0x53, 0x65, 0x68, 0x65, 0x6C, 0x6C, 0x6F, 0x50, 0x4B, 0x01, 0x02, 0x14, 0x03, 0x14,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0x86, 0xA6, 0x10, 0x36, 0x05, 0x00,
        0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x09, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x80, 0x01, 0x00, 0x00, 0x00, 0x00, 0x68, 0x65, 0x6C, 0x6C, 0x6F, 0x2E,
        0x74, 0x78, 0x74, // central dir entry
        0x55, 0x54, // extra field ID
        0x01, 0x00, // extra field length
        0x04, // extra field content (extended timestamp flags)
        0x50, 0x4B, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x3C, 0x00, 0x00,
        0x00, 0x35, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    let mut archive = ZipArchive::new(Cursor::new(zip)).expect("couldn't open test zip file");

    let mut extended_timestamp_field_found = false;
    for field in archive.by_name("hello.txt").unwrap().extra_data_fields() {
        if let zip::ExtraField::ExtendedTimestamp(ts) = field {
            extended_timestamp_field_found = true;
            assert!(ts.mod_time().is_none());
            assert!(ts.ac_time().is_none());
            assert!(ts.cr_time().is_none());
        }
    }
    assert!(extended_timestamp_field_found);
}
