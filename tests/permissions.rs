//! Test related to permissions

#[test]
fn test_write_symlink() {
    // if we specify a symlink, the zip_file should be forced to Unix
    use std::io::Cursor;
    use std::io::Write;
    use zip::CompressionMethod::Stored;
    use zip::HasZipMetadata; // We use the trait here
    use zip::System;
    use zip::ZipArchive;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;
    let perms: u32 = 0o755;

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(Stored)
        .unix_permissions(perms);

    let filename = format!("test_{perms}.txt");
    writer.start_file(&filename, options).unwrap();
    writer.write_all(b"content").unwrap();

    let symlink_name = format!("test_{perms}_symlink.txt");
    writer
        .add_symlink(&symlink_name, &filename, options)
        .unwrap();

    // Write and read back
    let bytes = writer.finish().unwrap().into_inner();
    let mut reader = ZipArchive::new(Cursor::new(bytes)).unwrap();

    {
        // test the normal file
        let file = reader.by_name(&filename).unwrap();
        #[cfg(windows)]
        let system = Sytem::Dos;
        #[cfg(not(windows))]
        let system = System::Unix;
        // The system of this file depends on the plateform
        assert_eq!(file.get_metadata().system, system);

        // Windows: `unix_mode()` gives the default mode
        #[cfg(windows)]
        assert_eq!(file.unix_mode().unwrap(), 0o100000 | 0o664);

        // Not Windows: Permissions are correctly saved as unix perms
        #[cfg(not(windows))]
        assert_eq!(file.unix_mode().unwrap(), 0o100000 | perms);
    }

    let symlink_file = reader.by_name(&symlink_name).unwrap();
    assert_eq!(symlink_file.get_metadata().system, System::Unix);
    // Windows: The ZipFile is forced to Unix so the permissions are also correctly saved
    // Not Windows: Permissions are correctly saved as unix perms
    assert_eq!(symlink_file.unix_mode().unwrap(), 0o120000 | perms);
}

#[test]
fn unix_mode_dos() {
    use std::io::Cursor;
    use zip::ZipArchive;
    // https://github.com/zip-rs/zip2/issues/737
    // unix_mode_0x081A4000.zip
    let data = [
        0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0x85,
        0x11, 0x4a, 0x0d, 0x0b, 0x00, 0x00, 0x00, 0x0b, 0x00, 0x00, 0x00, 0x0d, 0x00, 0x00, 0x00,
        0x74, 0x65, 0x73, 0x74, 0x5f, 0x66, 0x69, 0x6c, 0x65, 0x2e, 0x74, 0x78, 0x74, 0x68, 0x65,
        0x6c, 0x6c, 0x6f, 0x20, 0x77, 0x6f, 0x72, 0x6c, 0x64, 0x50, 0x4b, 0x01, 0x02, 0x2d, 0x00,
        0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0x85, 0x11, 0x4a, 0x0d, 0x0b,
        0x00, 0x00, 0x00, 0x0b, 0x00, 0x00, 0x00, // file name length
        0x0d, 0x00, // extra field length
        0x00, 0x00, // file comment length
        0x00, 0x00, // disk number start
        0x00, 0x00, // internal file attributes
        0x00, 0x00, // external file attributes
        0x00, 0x40, 0x1a, 0x08, // relative offset of local header
        0x00, 0x00, 0x00, 0x00, // filename: test_file.txt
        0x74, 0x65, 0x73, 0x74, 0x5f, 0x66, 0x69, 0x6c, 0x65, 0x2e, 0x74, 0x78, 0x74,
        // end central header
        0x50, 0x4b, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x3b, 0x00, 0x00,
        0x00, 0x36, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    let mut archive = ZipArchive::new(Cursor::new(data)).unwrap();
    for i in 0..archive.len() {
        let file = archive.by_index(i).unwrap();
        assert_eq!(
            file.unix_mode(),
            Some(0o100664),
            "file {:?} has unix_mode {:?}",
            file.name(),
            file.unix_mode()
        );
    }
}

#[test]
fn test_set_external_attributes() {
    use std::io::Cursor;
    use std::io::Write;
    use zip::CompressionMethod::Stored;
    use zip::HasZipMetadata; // We use the trait here
    use zip::ZipArchive;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;
    let attr: u32 = 0x123456;

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(Stored)
        .external_attributes(attr);

    let filename = format!("test_{:?}.txt", attr);
    writer.start_file(&filename, options).unwrap();
    writer.write_all(b"content").unwrap();

    // Write and read back
    let bytes = writer.finish().unwrap().into_inner();
    let mut reader = ZipArchive::new(Cursor::new(bytes)).unwrap();

    let file = reader.by_index(0).unwrap();
    assert_eq!(
        file.get_metadata().external_attributes,
        attr,
        "External attributes mismatch for {:?}",
        attr
    );
}
