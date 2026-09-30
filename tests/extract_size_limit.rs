//! Only on little endian because we cannot use fs with miri CI
#![cfg(all(target_endian = "little", not(miri)))]

use std::io::{Cursor, Write};

use tempfile::TempDir;
use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

fn stored_archive(parts: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in parts {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn open(bytes: &[u8]) -> ZipArchive<Cursor<&[u8]>> {
    ZipArchive::new(Cursor::new(bytes)).unwrap()
}

#[test]
fn extract_with_size_limit_rejects_before_writing() {
    let bytes = stored_archive(&[("a.txt", b"0123456789"), ("b.txt", b"0123456789")]);
    let mut archive = open(&bytes);
    let declared = archive.decompressed_size().unwrap();
    assert_eq!(declared, 20);

    let dest = TempDir::with_prefix("zip_size_limit_reject").unwrap();
    let out = dest.path().join("out");
    let err = archive
        .extract_with_size_limit(&out, 19)
        .expect_err("declared size is above the limit");

    assert!(matches!(
        err,
        ZipError::DecompressedSizeLimitExceeded {
            size: 20,
            limit: 19
        }
    ));
    assert!(!out.exists(), "rejection must not create the destination");
}

#[test]
fn extract_with_size_limit_allows_an_equal_total_and_extract_stays_unlimited() {
    let bytes = stored_archive(&[("a.txt", b"0123456789"), ("b.txt", b"0123456789")]);

    let limited = TempDir::with_prefix("zip_size_limit_equal").unwrap();
    let limited_out = limited.path().join("out");
    open(&bytes)
        .extract_with_size_limit(&limited_out, 20)
        .unwrap();
    assert_eq!(
        std::fs::read(limited_out.join("a.txt")).unwrap(),
        b"0123456789"
    );

    let plain = TempDir::with_prefix("zip_size_limit_plain").unwrap();
    let plain_out = plain.path().join("out");
    open(&bytes).extract(&plain_out).unwrap();
    assert_eq!(
        std::fs::read(plain_out.join("b.txt")).unwrap(),
        b"0123456789"
    );
}

#[test]
fn extract_with_size_limit_rejects_an_unknown_total() {
    let bytes = include_bytes!("data/data_descriptor.zip");
    let mut archive = open(bytes);
    assert_eq!(archive.decompressed_size(), None);

    let dest = TempDir::with_prefix("zip_size_limit_unknown").unwrap();
    let out = dest.path().join("out");
    let err = archive
        .extract_with_size_limit(&out, u64::MAX)
        .expect_err("a limit cannot be checked when the declared size is unknown");

    assert!(matches!(err, ZipError::DecompressedSizeUnknown));
    assert!(!out.exists());
}
