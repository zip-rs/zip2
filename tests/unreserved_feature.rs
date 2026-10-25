//! unreserved feature tests

#[cfg(not(feature = "unreserved"))]
#[test]
fn test_unreserved_feature_disabled() {
    use std::io::Cursor;
    use zip::write::{FileOptions, ZipFileBuilder};
    use zip::{CompressionMethod, ZipWriter};

    let buff = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buff);

    let mut options = FileOptions::default().compression_method(CompressionMethod::Stored);

    // Custom extra-field payload.
    // Replace this with your own bytes.
    let payload: Box<[u8]> = vec![
        0x01, // Version
        0x02, // UID length
        0xE8, 0x03, // UID = 1000
        0x02, // GID length
        0xE8, 0x03, // GID = 1000
    ]
    .into_boxed_slice();

    // Add extra field ID 0x7875 to the local header
    // and central directory.
    options.add_extra_field(0x7875, payload, false).unwrap();

    let res = zip.start_file("hello.txt", options.clone());
    assert!(res.is_err()); // Extra data header ID 030837 (0x7875) requires crate feature
    assert!(ZipFileBuilder::new("prepared.txt", options).is_err());
}

#[cfg(feature = "unreserved")]
#[test]
fn test_unreserved_feature_enabled() {
    use std::io::{Cursor, Read, Write};
    use zip::write::{FileOptions, ZipFileBuilder};
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    let mut buff = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(&mut buff));

    let mut options = FileOptions::default().compression_method(CompressionMethod::Stored);

    // Custom extra-field payload.
    // Replace this with your own bytes.
    let payload: Box<[u8]> = vec![
        0x01, // Version
        0x02, // UID length
        0xE8, 0x03, // UID = 1000
        0x02, // GID length
        0xE8, 0x03, // GID = 1000
    ]
    .into_boxed_slice();

    // Add extra field ID 0x7875 to the local header
    // and central directory.
    options.add_extra_field(0x7875, payload, false).unwrap();

    let res = zip.start_file("hello.txt", options.clone());
    assert!(res.is_ok()); // does not error
    zip.write_all(b"Hello from Rust!\n").unwrap();

    let mut prepared = ZipFileBuilder::new("prepared.txt", options).unwrap();
    prepared.write_all(b"Prepared file\n").unwrap();
    zip.add_prepared_file(prepared.finish().unwrap()).unwrap();
    zip.finish().unwrap();

    let mut archive = ZipArchive::new(Cursor::new(buff)).unwrap();
    assert_eq!(archive.len(), 2);
    let mut file = archive.by_name("prepared.txt").unwrap();
    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();
    assert_eq!(contents, "Prepared file\n");
    assert!(
        file.extra_data()
            .unwrap()
            .windows(11)
            .any(|field| { field == [0x75, 0x78, 7, 0, 1, 2, 0xE8, 3, 2, 0xE8, 3] })
    );
}

// Raw copy should work with or without the feature
#[test]
fn test_unreserved_feature_raw_copy() {
    use std::io::Cursor;

    let file = [
        0x50, 0x4B, 0x03, 0x04, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7B, 0x9F, 0x54, 0x5D, 0xCB,
        0x93, 0xE8, 0xDA, 0x11, 0x00, 0x00, 0x00, 0x11, 0x00, 0x00, 0x00, 0x09, 0x00, 0x0B, 0x00,
        0x68, 0x65, 0x6C, 0x6C, 0x6F, 0x2E, 0x74, 0x78, 0x74, 0x75, 0x78, 0x07, 0x00, 0x01, 0x02,
        0xE8, 0x03, 0x02, 0xE8, 0x03, 0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x20, 0x66, 0x72, 0x6F, 0x6D,
        0x20, 0x52, 0x75, 0x73, 0x74, 0x21, 0x0A, 0x50, 0x4B, 0x01, 0x02, 0x0A, 0x03, 0x0A, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x7B, 0x9F, 0x54, 0x5D, 0xCB, 0x93, 0xE8, 0xDA, 0x11, 0x00, 0x00,
        0x00, 0x11, 0x00, 0x00, 0x00, 0x09, 0x00, 0x0B, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0xA4, 0x81, 0x00, 0x00, 0x00, 0x00, 0x68, 0x65, 0x6C, 0x6C, 0x6F, 0x2E, 0x74,
        0x78, 0x74, 0x75, 0x78, 0x07, 0x00, 0x01, 0x02, 0xE8, 0x03, 0x02, 0xE8, 0x03, 0x50, 0x4B,
        0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x42, 0x00, 0x00, 0x00, 0x43,
        0x00, 0x00, 0x00, 0x00, 0x00,
    ]
    .as_ref();

    let mut input = zip::ZipArchive::new(Cursor::new(file)).unwrap();
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..input.len() {
        let file = input.by_index_raw(i).unwrap();
        let res = output.raw_copy_file(file);
        assert!(res.is_ok());
    }
    output.finish().unwrap();
}
