//! An entry whose data is larger than its declared uncompressed size must fail to read once
//! the declared size is exceeded, so that the declared sizes (and therefore
//! `ZipArchive::decompressed_size`) bound what reading and extracting produce.

use std::io::{Cursor, Read, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

// Large enough that reading past the declared size would be obvious; smaller under miri,
// which is slow.
const REAL_SIZE: usize = if cfg!(miri) { 1 << 12 } else { 1 << 20 };
const DECLARED_SIZE: u32 = 10;

/// Build an archive with one entry of `REAL_SIZE` zero bytes, then patch the uncompressed
/// size in both the local header and the central directory header to `DECLARED_SIZE`.
/// The CRC-32 still matches the real data.
fn lying_archive(method: CompressionMethod) -> Vec<u8> {
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    w.start_file(
        "a.bin",
        SimpleFileOptions::default().compression_method(method),
    )
    .unwrap();
    w.write_all(&vec![0u8; REAL_SIZE]).unwrap();
    let mut data = w.finish().unwrap().into_inner();

    let local = data.windows(4).position(|s| s == b"PK\x03\x04").unwrap();
    let central = data.windows(4).position(|s| s == b"PK\x01\x02").unwrap();
    data[local + 22..local + 26].copy_from_slice(&DECLARED_SIZE.to_le_bytes());
    data[central + 24..central + 28].copy_from_slice(&DECLARED_SIZE.to_le_bytes());
    data
}

fn methods() -> Vec<CompressionMethod> {
    vec![
        CompressionMethod::Stored,
        #[cfg(feature = "_deflate-any")]
        CompressionMethod::Deflated,
    ]
}

#[test]
fn read_fails_past_declared_size() {
    for method in methods() {
        let mut archive = ZipArchive::new(Cursor::new(lying_archive(method))).unwrap();
        assert_eq!(archive.decompressed_size(), Some(u128::from(DECLARED_SIZE)));

        let mut file = archive.by_index(0).unwrap();
        assert_eq!(file.size(), u64::from(DECLARED_SIZE));
        let mut buf = [0u8; 4096];
        let mut total = 0u64;
        let err = loop {
            match file.read(&mut buf) {
                Ok(0) => panic!("{method:?}: read {total} bytes without an error"),
                Ok(n) => total += n as u64,
                Err(e) => break e,
            }
        };
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{method:?}");
        assert!(
            total <= u64::from(DECLARED_SIZE),
            "{method:?}: {total} bytes"
        );
        drop(file);

        let mut out = Vec::new();
        assert!(archive.by_index(0).unwrap().read_to_end(&mut out).is_err());
        assert!(
            out.len() <= DECLARED_SIZE as usize + 1,
            "{method:?}: {}",
            out.len()
        );
    }
}

// uses the file system, which miri's isolation does not allow
#[cfg(not(miri))]
#[test]
fn extract_fails_past_declared_size() {
    for method in methods() {
        let mut archive = ZipArchive::new(Cursor::new(lying_archive(method))).unwrap();
        let dir = tempfile::TempDir::new().unwrap();
        assert!(archive.extract(dir.path()).is_err(), "{method:?}");
        let written = std::fs::metadata(dir.path().join("a.bin")).map_or(0, |m| m.len());
        assert!(
            written <= u64::from(DECLARED_SIZE),
            "{method:?}: {written} bytes"
        );
    }
}

#[test]
fn exact_declared_size_still_reads() {
    for method in methods() {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file(
            "a.bin",
            SimpleFileOptions::default().compression_method(method),
        )
        .unwrap();
        w.write_all(&vec![7u8; REAL_SIZE]).unwrap();
        let data = w.finish().unwrap().into_inner();
        let mut archive = ZipArchive::new(Cursor::new(data)).unwrap();
        let mut out = Vec::new();
        archive.by_index(0).unwrap().read_to_end(&mut out).unwrap();
        assert_eq!(out.len(), REAL_SIZE, "{method:?}");
    }
}
