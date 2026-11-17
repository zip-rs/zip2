use std::io::{Cursor, Read};
use zip::unstable::format::blocks::{ZipCentralEntryBlock, ZipLocalEntryBlock};

// Independent fixed records, including their signatures. Variable fields deliberately
// have nonzero lengths: parsing the fixed record must leave their bytes untouched.
const CENTRAL: [u8; 46] = [
    0x50, 0x4b, 0x01, 0x02, 0x14, 0x03, 0x14, 0x00, 0x00, 0x08, 0x08, 0x00, 0x21, 0x43, 0x65, 0x87,
    0x78, 0x56, 0x34, 0x12, 0x04, 0x03, 0x02, 0x01, 0x08, 0x07, 0x06, 0x05, 0x04, 0x00, 0x02, 0x00,
    0x03, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0xed, 0x81, 0x0c, 0x0b, 0x0a, 0x09,
];
const LOCAL: [u8; 30] = [
    0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x08, 0x08, 0x00, 0x21, 0x43, 0x65, 0x87, 0x78, 0x56,
    0x34, 0x12, 0x04, 0x03, 0x02, 0x01, 0x08, 0x07, 0x06, 0x05, 0x04, 0x00, 0x02, 0x00,
];

#[test]
fn central_fixed_header_preserves_fields_and_variable_bytes() {
    let mut input = Cursor::new([CENTRAL.as_slice(), b"nameexcom"].concat());
    let header = ZipCentralEntryBlock::parse(&mut input).unwrap();
    // Copy packed fields before assertions so no unaligned references are formed.
    let versions = (header.version_made_by, header.version_to_extract);
    let encoding = (header.flags, header.compression_method);
    let timestamp = (header.last_mod_time, header.last_mod_date);
    let sizes = (
        header.crc32,
        header.compressed_size,
        header.uncompressed_size,
    );
    let lengths = (
        header.file_name_length,
        header.extra_field_length,
        header.file_comment_length,
    );
    let attributes = (
        header.disk_number,
        header.internal_file_attributes,
        header.external_file_attributes,
        header.offset,
    );
    assert_eq!(versions, (0x0314, 20));
    assert_eq!(encoding, (0x0800, 8));
    assert_eq!(timestamp, (0x4321, 0x8765));
    assert_eq!(sizes, (0x12345678, 0x01020304, 0x05060708));
    assert_eq!(lengths, (4, 2, 3));
    assert_eq!(attributes, (1, 2, 0x81ed0000, 0x090a0b0c));
    assert_eq!(input.position(), 46);
    let mut remainder = Vec::new();
    input.read_to_end(&mut remainder).unwrap();
    assert_eq!(remainder, b"nameexcom");
}

#[test]
fn local_fixed_header_preserves_fields_and_variable_bytes() {
    let mut input = Cursor::new([LOCAL.as_slice(), b"nameex"].concat());
    let header = ZipLocalEntryBlock::parse(&mut input).unwrap();
    let encoding = (
        header.version_made_by,
        header.flags,
        header.compression_method,
    );
    let timestamp = (header.last_mod_time, header.last_mod_date);
    let sizes = (
        header.crc32,
        header.compressed_size,
        header.uncompressed_size,
    );
    let lengths = (header.file_name_length, header.extra_field_length);
    assert_eq!(encoding, (20, 0x0800, 8));
    assert_eq!(timestamp, (0x4321, 0x8765));
    assert_eq!(sizes, (0x12345678, 0x01020304, 0x05060708));
    assert_eq!(lengths, (4, 2));
    assert_eq!(input.position(), 30);
    let mut remainder = Vec::new();
    input.read_to_end(&mut remainder).unwrap();
    assert_eq!(remainder, b"nameex");
}

#[test]
fn fixed_headers_reject_every_truncated_prefix() {
    for length in 0..CENTRAL.len() {
        assert!(
            ZipCentralEntryBlock::parse(&mut &CENTRAL[..length]).is_err(),
            "central prefix {length}"
        );
    }
    for length in 0..LOCAL.len() {
        assert!(
            ZipLocalEntryBlock::parse(&mut &LOCAL[..length]).is_err(),
            "local prefix {length}"
        );
    }
}

#[test]
fn fixed_headers_reject_wrong_magic() {
    let mut central = CENTRAL;
    central[0] = 0;
    assert!(ZipCentralEntryBlock::parse(&mut central.as_slice()).is_err());
    let mut local = LOCAL;
    local[0] = 0;
    assert!(ZipLocalEntryBlock::parse(&mut local.as_slice()).is_err());
}

#[test]
fn fixed_headers_accept_unsized_readers() {
    let mut central = CENTRAL.as_slice();
    let reader: &mut dyn Read = &mut central;
    assert!(ZipCentralEntryBlock::parse(reader).is_ok());
    assert!(central.is_empty());
    let mut local = LOCAL.as_slice();
    let reader: &mut dyn Read = &mut local;
    assert!(ZipLocalEntryBlock::parse(reader).is_ok());
    assert!(local.is_empty());
}
