#![cfg(feature = "rayon")]

use std::fs::File;
use std::io::{Cursor, Read, Write};
use tempfile::tempdir;
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;
use zip::write::{PreparedZipFile, ZipWriter};

#[test]
fn test_par_append_data_in_memory() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::default());

    let entries: Vec<(String, SimpleFileOptions, Vec<u8>)> = (0..50)
        .map(|i| {
            (
                format!("file_{i:03}.txt"),
                options,
                format!(
                    "Payload of file {i} with repetitive data: {}",
                    "A".repeat(500)
                )
                .into_bytes(),
            )
        })
        .collect();

    writer.par_append_data(entries).unwrap();
    let mut archive = writer.finish_into_readable().unwrap();

    assert_eq!(archive.len(), 50);
    for i in 0..50 {
        let name = format!("file_{i:03}.txt");
        let mut file = archive.by_name(&name).unwrap();
        let mut content = Vec::new();
        file.read_to_end(&mut content).unwrap();
        let expected = format!(
            "Payload of file {i} with repetitive data: {}",
            "A".repeat(500)
        )
        .into_bytes();
        assert_eq!(content, expected);
    }
}

#[test]
fn test_par_append_data_batches() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();

    let entries: Vec<(String, SimpleFileOptions, Vec<u8>)> = (0..35)
        .map(|i| (format!("batch_{i}.dat"), options, vec![i as u8; 256]))
        .collect();

    writer.par_append_data_batches(&entries, 10).unwrap();
    let mut archive = writer.finish_into_readable().unwrap();

    assert_eq!(archive.len(), 35);
    for i in 0..35 {
        let name = format!("batch_{i}.dat");
        let mut file = archive.by_name(&name).unwrap();
        let mut content = Vec::new();
        file.read_to_end(&mut content).unwrap();
        assert_eq!(content, vec![i as u8; 256]);
    }
}

#[test]
fn test_par_append_from_paths() {
    let dir = tempdir().unwrap();
    let mut entries = Vec::new();
    let options = SimpleFileOptions::default();

    for i in 0..20 {
        let file_path = dir.path().join(format!("disk_file_{i}.txt"));
        let payload = format!("Disk content {i} - {}", "TEST".repeat(100));
        let mut f = File::create(&file_path).unwrap();
        f.write_all(payload.as_bytes()).unwrap();
        entries.push((format!("archived_{i}.txt"), file_path, options));
    }

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.par_append_from_paths(entries).unwrap();
    let mut archive = writer.finish_into_readable().unwrap();

    assert_eq!(archive.len(), 20);
    for i in 0..20 {
        let name = format!("archived_{i}.txt");
        let mut file = archive.by_name(&name).unwrap();
        let mut content = String::new();
        file.read_to_string(&mut content).unwrap();
        let expected = format!("Disk content {i} - {}", "TEST".repeat(100));
        assert_eq!(content, expected);
    }
}

#[test]
fn test_par_append_from_paths_batches() {
    let dir = tempdir().unwrap();
    let mut entries = Vec::new();
    let options = SimpleFileOptions::default();

    for i in 0..15 {
        let file_path = dir.path().join(format!("batch_disk_{i}.bin"));
        let payload = vec![i as u8; 512];
        let mut f = File::create(&file_path).unwrap();
        f.write_all(&payload).unwrap();
        entries.push((format!("archived_batch_{i}.bin"), file_path, options));
    }

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.par_append_from_paths_batches(&entries, 5).unwrap();
    let mut archive = writer.finish_into_readable().unwrap();

    assert_eq!(archive.len(), 15);
    for i in 0..15 {
        let name = format!("archived_batch_{i}.bin");
        let mut file = archive.by_name(&name).unwrap();
        let mut content = Vec::new();
        file.read_to_end(&mut content).unwrap();
        assert_eq!(content, vec![i as u8; 512]);
    }
}

#[test]
fn test_prepared_zip_file_accessors() {
    let options = SimpleFileOptions::default();
    let payload = b"Accessor test payload";
    let prepared = PreparedZipFile::from_data("test_meta.txt", options, payload).unwrap();

    assert_eq!(prepared.file_name_raw(), b"test_meta.txt");
    assert_eq!(prepared.uncompressed_size(), payload.len() as u64);
    assert!(prepared.compressed_size() > 0);
    assert_ne!(prepared.crc32(), 0);

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.add_prepared_files([prepared]).unwrap();
    let mut archive = writer.finish_into_readable().unwrap();

    let mut file = archive.by_name("test_meta.txt").unwrap();
    let mut content = Vec::new();
    file.read_to_end(&mut content).unwrap();
    assert_eq!(content, payload);
}

#[test]
fn test_par_append_duplicate_name_error() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();

    let entries = vec![
        ("dup.txt".to_string(), options, b"one".to_vec()),
        ("dup.txt".to_string(), options, b"two".to_vec()),
    ];

    let result = writer.par_append_data(entries);
    assert!(result.is_err());
}
