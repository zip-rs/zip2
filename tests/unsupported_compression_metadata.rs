use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;
use zip::{HasZipMetadata, ZipArchive, ZipWriter};

fn archive_with_unsupported_compression() -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file("entry.txt", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"content").unwrap();
    let mut bytes = writer.finish().unwrap().into_inner();

    let local_header = bytes
        .windows(4)
        .position(|window| window == b"PK\x03\x04")
        .unwrap();
    let central_header = bytes
        .windows(4)
        .position(|window| window == b"PK\x01\x02")
        .unwrap();
    let unsupported_method = 222u16.to_le_bytes();
    bytes[local_header + 8..local_header + 10].copy_from_slice(&unsupported_method);
    bytes[central_header + 10..central_header + 12].copy_from_slice(&unsupported_method);

    bytes
}

#[test]
fn metadata_is_available_for_unsupported_compression() {
    let mut archive = ZipArchive::new(Cursor::new(archive_with_unsupported_compression())).unwrap();

    let file = archive.by_name("entry.txt").unwrap();
    assert!(file.get_metadata().compressed_size > 0);
    drop(file);

    let file = archive.by_index(0).unwrap();
    assert!(file.get_metadata().compressed_size > 0);
    drop(file);

    let file = archive.by_name_raw(b"entry.txt").unwrap();
    assert!(file.get_metadata().compressed_size > 0);
}
