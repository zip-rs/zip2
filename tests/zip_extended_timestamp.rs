//! Tests on extended timestamp

use zip::{ZipArchive, read::read_zipfile_from_stream};

#[test]
fn test_extended_timestamp() {
    use std::io::Cursor;

    let v = include_bytes!("../tests/data/extended_timestamp.zip");
    let mut archive = ZipArchive::new(Cursor::new(v)).expect("couldn't open test zip file");

    let mut is_extended_timestamp_extra_field = false;
    for field in archive.by_name("test.txt").unwrap().extra_data_fields() {
        if let zip::ExtraField::ExtendedTimestamp(ts) = field {
            is_extended_timestamp_extra_field = true;
            assert!(ts.ac_time().is_none());
            assert!(ts.cr_time().is_none());
            assert_eq!(ts.mod_time().unwrap(), 1714635025);
        }
    }
    assert!(is_extended_timestamp_extra_field);
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

    let mut is_extended_timestamp_extra_field = false;
    for field in file.extra_data_fields() {
        if let zip::ExtraField::ExtendedTimestamp(ts) = field {
            is_extended_timestamp_extra_field = true;
            assert_eq!(ts.ac_time(), Some(1714635039));
            assert_eq!(ts.cr_time(), None);
            assert_eq!(ts.mod_time(), Some(1714635025));
        }
    }
    assert!(is_extended_timestamp_extra_field);
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
        0x74, 0x78, 0x74, 0x55, 0x54, 0x01, 0x00, 0x04, 0x50, 0x4B, 0x05, 0x06, 0x00, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x01, 0x00, 0x3C, 0x00, 0x00, 0x00, 0x35, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    let mut archive = ZipArchive::new(Cursor::new(zip)).expect("couldn't open test zip file");

    let mut is_extended_timestamp_extra_field = false;
    for field in archive.by_name("hello.txt").unwrap().extra_data_fields() {
        if let zip::ExtraField::ExtendedTimestamp(ts) = field {
            is_extended_timestamp_extra_field = true;
            assert!(ts.ac_time().is_none());
            assert!(ts.cr_time().is_none());
            assert!(ts.mod_time().is_none());
        }
    }
    assert!(is_extended_timestamp_extra_field);
}
