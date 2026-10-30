#![cfg(feature = "rayon")]
#![allow(deprecated)]

use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::num::NonZeroUsize;
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

    writer
        .par_append_data_batches(&entries, NonZeroUsize::new(10).unwrap())
        .unwrap();
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
    writer
        .par_append_from_paths_batches(&entries, NonZeroUsize::new(5).unwrap())
        .unwrap();
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

#[test]
fn test_par_append_empty_inputs() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let empty_data: Vec<(String, SimpleFileOptions, Vec<u8>)> = Vec::new();
    writer.par_append_data(empty_data).unwrap();

    let empty_data_slice: &[(String, SimpleFileOptions, Vec<u8>)] = &[];
    writer
        .par_append_data_batches(empty_data_slice, NonZeroUsize::new(10).unwrap())
        .unwrap();

    let empty_paths: Vec<(String, std::path::PathBuf, SimpleFileOptions)> = Vec::new();
    writer.par_append_from_paths(empty_paths).unwrap();

    let empty_paths_slice: &[(String, std::path::PathBuf, SimpleFileOptions)] = &[];
    writer
        .par_append_from_paths_batches(empty_paths_slice, NonZeroUsize::new(10).unwrap())
        .unwrap();

    let archive = writer.finish_into_readable().unwrap();
    assert_eq!(archive.len(), 0);
}

#[test]
fn test_par_append_zero_byte_empty_file() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let opts_stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let opts_deflated = SimpleFileOptions::default().compression_method(CompressionMethod::DEFLATE);

    let entries = vec![
        ("empty_stored.dat", opts_stored, b"".as_slice()),
        ("empty_deflated.dat", opts_deflated, b"".as_slice()),
    ];

    writer.par_append_data(entries).unwrap();
    let mut archive = writer.finish_into_readable().unwrap();

    assert_eq!(archive.len(), 2);
    for name in &["empty_stored.dat", "empty_deflated.dat"] {
        let mut file = archive.by_name(name).unwrap();
        assert_eq!(file.size(), 0);
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).unwrap();
        assert!(buf.is_empty());
    }
}

#[test]
fn test_par_append_unicode_and_special_paths() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();

    let entries = vec![
        (
            "türkçe_şeker_çilek_öğrenci.txt",
            options,
            b"Turkce icerik".as_slice(),
        ),
        (
            "nested/sub_dir/deep_file.log",
            options,
            b"Nested log".as_slice(),
        ),
        ("🦀_emoji_file_🇹🇷.bin", options, b"Emoji payload".as_slice()),
        (
            "path with spaces and symbols #1!.csv",
            options,
            b"col1,col2".as_slice(),
        ),
    ];

    writer.par_append_data(entries).unwrap();
    let mut archive = writer.finish_into_readable().unwrap();

    assert_eq!(archive.len(), 4);
    {
        let mut f1 = archive.by_name("türkçe_şeker_çilek_öğrenci.txt").unwrap();
        let mut c1 = Vec::new();
        f1.read_to_end(&mut c1).unwrap();
        assert_eq!(c1, b"Turkce icerik");
    }
    {
        let mut f2 = archive.by_name("nested/sub_dir/deep_file.log").unwrap();
        let mut c2 = Vec::new();
        f2.read_to_end(&mut c2).unwrap();
        assert_eq!(c2, b"Nested log");
    }
    {
        let mut f3 = archive.by_name("🦀_emoji_file_🇹🇷.bin").unwrap();
        let mut c3 = Vec::new();
        f3.read_to_end(&mut c3).unwrap();
        assert_eq!(c3, b"Emoji payload");
    }
}

#[test]
fn test_par_append_batch_size_edge_cases() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();

    let entries: Vec<(String, SimpleFileOptions, Vec<u8>)> = (0..7)
        .map(|i| (format!("item_{i}.txt"), options, vec![i as u8; 32]))
        .collect();

    // batch_size = 1 (single-item chunks)
    writer
        .par_append_data_batches(&entries[..3], NonZeroUsize::new(1).unwrap())
        .unwrap();

    // batch_size = 100 (> entries len)
    writer
        .par_append_data_batches(&entries[3..5], NonZeroUsize::new(100).unwrap())
        .unwrap();

    // batch_size = 3 (non-divisor)
    writer
        .par_append_data_batches(&entries[5..], NonZeroUsize::new(3).unwrap())
        .unwrap();

    let mut archive = writer.finish_into_readable().unwrap();
    assert_eq!(archive.len(), 7);
    for i in 0..7 {
        let mut file = archive.by_name(&format!("item_{i}.txt")).unwrap();
        let mut content = Vec::new();
        file.read_to_end(&mut content).unwrap();
        assert_eq!(content, vec![i as u8; 32]);
    }
}

#[test]
fn test_par_append_duplicate_pre_existing_atomicity() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();

    // Initially write "alpha.txt"
    writer.start_file("alpha.txt", options).unwrap();
    writer.write_all(b"alpha").unwrap();

    // Now try to append batch containing "beta.txt" and duplicate "alpha.txt"
    let entries = vec![
        ("beta.txt", options, b"beta".as_slice()),
        ("alpha.txt", options, b"alpha_dup".as_slice()),
    ];

    let result = writer.par_append_data_atomic(entries);
    assert!(result.is_err());

    // Because pre-validation caught the collision, "beta.txt" was NOT written!
    let mut archive = writer.finish_into_readable().unwrap();
    assert_eq!(archive.len(), 1);
    assert!(archive.by_name("alpha.txt").is_ok());
    assert!(archive.by_name("beta.txt").is_err());
}

#[test]
fn test_par_append_nonexistent_disk_path_atomicity() {
    let dir = tempdir().unwrap();
    let valid_path = dir.path().join("good.txt");
    std::fs::write(&valid_path, b"good content").unwrap();

    let invalid_path = dir.path().join("does_not_exist_xyz.bin");
    let options = SimpleFileOptions::default();

    let entries = vec![
        ("good.txt".to_string(), valid_path, options),
        ("bad.txt".to_string(), invalid_path, options),
    ];

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let result = writer.par_append_from_paths_atomic(entries);
    assert!(result.is_err());

    // Pristine archive: 0 entries committed
    let archive = writer.finish_into_readable().unwrap();
    assert_eq!(archive.len(), 0);
}

#[test]
fn test_par_append_scoped_single_thread_pool() {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();

    pool.install(|| {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();

        let entries: Vec<(String, SimpleFileOptions, Vec<u8>)> = (0..10)
            .map(|i| (format!("single_pool_{i}.dat"), options, vec![i as u8; 64]))
            .collect();

        writer.par_append_data(entries).unwrap();
        let archive = writer.finish_into_readable().unwrap();
        assert_eq!(archive.len(), 10);
    });
}

#[test]
fn test_par_append_with_capacity_and_backpressure() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();

    let entries: Vec<(String, SimpleFileOptions, Vec<u8>)> = (0..20)
        .map(|i| (format!("capacity_{i}.bin"), options, vec![i as u8; 128]))
        .collect();

    // Test with tight channel capacity = 1 to exercise strict backpressure
    writer
        .par_append_data_with_capacity(entries, NonZeroUsize::new(1).unwrap())
        .unwrap();

    let dir = tempdir().unwrap();
    let mut path_entries = Vec::new();
    for i in 0..5 {
        let p = dir.path().join(format!("from_path_cap_{i}.bin"));
        std::fs::write(&p, vec![i as u8; 64]).unwrap();
        path_entries.push((format!("from_path_cap_{i}.bin"), p, options));
    }

    writer
        .par_append_from_paths_with_capacity(path_entries, NonZeroUsize::new(2).unwrap())
        .unwrap();

    let mut archive = writer.finish_into_readable().unwrap();
    assert_eq!(archive.len(), 25);
    let mut buf = Vec::new();
    let mut f = archive.by_name("capacity_5.bin").unwrap();
    f.read_to_end(&mut buf).unwrap();
    assert_eq!(buf, vec![5u8; 128]);
}

#[test]
fn test_par_append_cp437_raw_name_override() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();

    // Two entries with different string names, but both override to the SAME raw byte name!
    let entry1 = (
        "utf8_one.txt",
        options.name(b"shared_raw.bin"),
        b"data1".as_slice(),
    );
    let entry2 = (
        "utf8_two.txt",
        options.name(b"shared_raw.bin"),
        b"data2".as_slice(),
    );

    let result = writer.par_append_data(vec![entry1, entry2]);
    assert!(
        result.is_err(),
        "Collision on options.name CP437 raw bytes must be rejected"
    );
}
