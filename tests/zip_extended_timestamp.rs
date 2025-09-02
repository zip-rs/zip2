use std::io;
use zip::ZipArchive;

#[test]
fn test_extended_timestamp() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("../tests/data/extended_timestamp.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    for field in archive.by_name("test.txt").unwrap().extra_data_fields() {
        if let zip::ExtraField::ExtendedTimestamp(ts) = field {
            assert!(ts.ac_time().is_none());
            assert!(ts.cr_time().is_none());
            assert_eq!(ts.mod_time().unwrap(), 1714635025);
        }
    }
}

#[test]
fn test_extended_timestamp_empty_field() {
    use zip::extra_fields::ExtendedTimestamp;
    use std::io::Cursor;
    
    // Test with empty field (len = 0) - should return error instead of panicking
    let mut cursor = Cursor::new(vec![]);
    let result = ExtendedTimestamp::try_from_reader(&mut cursor, 0);
    
    assert!(result.is_err());
    if let Err(zip::result::ZipError::UnsupportedArchive(msg)) = result {
        assert!(msg.contains("extended timestamp field is too small for flags"));
    } else {
        panic!("Expected UnsupportedArchive error");
    }
}

#[test]
fn test_extended_timestamp_insufficient_bytes() {
    use zip::extra_fields::ExtendedTimestamp;
    use std::io::Cursor;
    
    // Test with insufficient bytes for modification time
    // Flags indicate mod_time is present (0x01) but only 1 byte total length
    let mut cursor = Cursor::new(vec![0x01]); // flags byte only
    let result = ExtendedTimestamp::try_from_reader(&mut cursor, 1);
    
    assert!(result.is_err());
    if let Err(zip::result::ZipError::UnsupportedArchive(msg)) = result {
        assert!(msg.contains("insufficient bytes for modification time"));
    } else {
        panic!("Expected UnsupportedArchive error for insufficient bytes");
    }
}
