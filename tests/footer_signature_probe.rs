//! Private compatibility probe for the optional central-directory digital signature.

#![cfg(not(miri))]

use std::io::{Cursor, Write};
use zip::unstable::stream::ZipStreamReader;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

fn archive_with_signature(size_includes_signature: bool) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "file.txt",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(b"content").unwrap();
    let mut bytes = writer.finish().unwrap().into_inner();

    let eocd = bytes
        .windows(4)
        .rposition(|bytes| bytes == b"PK\x05\x06")
        .unwrap();
    let old_cd_size = u32::from_le_bytes(bytes[eocd + 12..eocd + 16].try_into().unwrap());
    let digital_signature = b"PK\x05\x05\x02\x00ok";
    bytes.splice(eocd..eocd, digital_signature.iter().copied());
    if size_includes_signature {
        bytes[eocd + digital_signature.len() + 12..eocd + digital_signature.len() + 16]
            .copy_from_slice(&(old_cd_size + digital_signature.len() as u32).to_le_bytes());
    }
    bytes
}

#[test]
fn strict_stream_accepts_a_sized_central_directory_digital_signature() {
    let bytes = archive_with_signature(true);
    assert_eq!(ZipArchive::new(Cursor::new(&bytes)).unwrap().len(), 1);
    let out = tempfile::tempdir().unwrap();
    ZipStreamReader::new(Cursor::new(&bytes))
        .enforce_header_consistency(true)
        .extract(out.path())
        .unwrap();
    assert_eq!(
        std::fs::read(out.path().join("file.txt")).unwrap(),
        b"content"
    );
}

#[test]
fn strict_stream_rejects_a_signature_missing_from_the_declared_size() {
    let bytes = archive_with_signature(false);
    let out = tempfile::tempdir().unwrap();
    assert!(
        ZipStreamReader::new(Cursor::new(&bytes))
            .enforce_header_consistency(true)
            .extract(out.path())
            .is_err()
    );
    assert!(!out.path().join("file.txt").exists());
}

#[test]
fn strict_stream_rejects_a_truncated_digital_signature() {
    let mut bytes = archive_with_signature(true);
    let signature = bytes
        .windows(4)
        .position(|bytes| bytes == b"PK\x05\x05")
        .unwrap();
    bytes[signature + 4..signature + 6].copy_from_slice(&u16::MAX.to_le_bytes());
    let out = tempfile::tempdir().unwrap();
    assert!(
        ZipStreamReader::new(Cursor::new(&bytes))
            .enforce_header_consistency(true)
            .extract(out.path())
            .is_err()
    );
    assert!(!out.path().join("file.txt").exists());
}
