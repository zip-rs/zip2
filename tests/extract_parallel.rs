//! Tests for parallel archive extraction engine.
#![cfg(all(not(miri), feature = "parallel"))]

use std::fs;
use std::io::{Cursor, Write};
use tempfile::TempDir;
use zip::read::ExtractOptions;
use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

#[test]
fn test_par_extract_in_memory_cursor() {
    let buf = Vec::new();
    let mut writer = ZipWriter::new(Cursor::new(buf));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    for i in 0..30 {
        let name = format!("folder/file_{i}.txt");
        writer.start_file(&name, options).unwrap();
        writer
            .write_all(format!("payload content for file {i}").as_bytes())
            .unwrap();
    }

    let zip_bytes = writer.finish().unwrap().into_inner();
    let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();

    let dest = TempDir::with_prefix("par_extract_mem").unwrap();
    archive.par_extract(dest.path()).unwrap();

    for i in 0..30 {
        let expected_path = dest.path().join(format!("folder/file_{i}.txt"));
        assert!(expected_path.exists());
        let content = fs::read_to_string(&expected_path).unwrap();
        assert_eq!(content, format!("payload content for file {i}"));
    }
}

#[test]
fn test_par_extract_from_file_path() {
    let temp_dir = TempDir::with_prefix("par_extract_fs_src").unwrap();
    let zip_path = temp_dir.path().join("archive.zip");
    let file = fs::File::create(&zip_path).unwrap();

    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    // Create directories and mixed files
    writer.add_directory("sub_alpha/", options).unwrap();
    writer.add_directory("sub_beta/", options).unwrap();

    for i in 0..40 {
        let folder = if i % 2 == 0 { "sub_alpha" } else { "sub_beta" };
        let name = format!("{folder}/data_{i}.bin");
        writer.start_file(&name, options).unwrap();
        let payload = vec![i as u8; 1024]; // 1 KiB each
        writer.write_all(&payload).unwrap();
    }

    let file = writer.finish().unwrap();
    drop(file);

    // Open file archive and extract in parallel
    let read_file = fs::File::open(&zip_path).unwrap();
    let mut archive = ZipArchive::new(read_file).unwrap();

    let dest = TempDir::with_prefix("par_extract_fs_dst").unwrap();
    archive.par_extract_from_path(&zip_path, dest.path()).unwrap();

    for i in 0..40 {
        let folder = if i % 2 == 0 { "sub_alpha" } else { "sub_beta" };
        let expected_path = dest.path().join(format!("{folder}/data_{i}.bin"));
        assert!(expected_path.exists());
        let data = fs::read(&expected_path).unwrap();
        assert_eq!(data.len(), 1024);
        assert!(data.iter().all(|&b| b == (i as u8)));
    }
}

#[test]
fn test_par_extract_with_size_limit_rejection() {
    let buf = Vec::new();
    let mut writer = ZipWriter::new(Cursor::new(buf));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    for i in 0..10 {
        writer.start_file(format!("file_{i}.txt"), options).unwrap();
        writer.write_all(b"1234567890").unwrap(); // 10 bytes each = 100 bytes total
    }

    let zip_bytes = writer.finish().unwrap().into_inner();
    let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();

    let dest = TempDir::with_prefix("par_extract_limit").unwrap();

    // Limit set to 50 bytes (total is 100 bytes) -> Must fail with DecompressedSizeLimitExceeded
    let err = archive
        .par_extract_with_options(dest.path(), ExtractOptions::default().with_size_limit(50))
        .expect_err("size limit exceeded must be rejected");

    assert!(matches!(
        err,
        ZipError::DecompressedSizeLimitExceeded { size: 100, limit: 50 }
    ));
}

#[test]
fn test_par_extract_duplicate_paths_safe() {
    let buf = Vec::new();
    let mut writer = ZipWriter::new(Cursor::new(buf));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    // Two distinct raw entries that resolve to the same normalized destination path
    writer.start_file("dup.txt", options).unwrap();
    writer.write_all(b"first entry").unwrap();

    writer.start_file("./dup.txt", options).unwrap();
    writer.write_all(b"second entry (later)").unwrap();

    let zip_bytes = writer.finish().unwrap().into_inner();
    let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();

    let dest = TempDir::with_prefix("par_extract_dup").unwrap();
    // Must succeed without file sharing violation or concurrent write race
    archive.par_extract(dest.path()).unwrap();

    let content = fs::read(dest.path().join("dup.txt")).unwrap();
    assert_eq!(content, b"second entry (later)");
}
