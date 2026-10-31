//! Tests for parallel archive extraction engine.
#![cfg(all(not(miri), feature = "rayon"))]

use std::fs;
use std::io::{Cursor, Write};
use tempfile::TempDir;
use zip::read::ExtractOptions;
use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

fn enabled_read_write_compression_methods() -> Vec<CompressionMethod> {
    vec![
        CompressionMethod::Stored,
        #[cfg(feature = "_deflate-any")]
        CompressionMethod::Deflated,
        #[cfg(feature = "_bzip2_any")]
        CompressionMethod::Bzip2,
        #[cfg(feature = "zstd")]
        CompressionMethod::Zstd,
        #[cfg(feature = "xz")]
        CompressionMethod::Xz,
        #[cfg(feature = "ppmd")]
        CompressionMethod::Ppmd,
    ]
}

#[test]
fn test_par_extract_in_memory_cursor() {
    for method in enabled_read_write_compression_methods() {
        let buf = Vec::new();
        let mut writer = ZipWriter::new(Cursor::new(buf));
        let options = SimpleFileOptions::default().compression_method(method);

        for i in 0..25 {
            let name = format!("folder_{method:?}/file_{i}.txt");
            writer.start_file(&name, options).unwrap();
            writer
                .write_all(format!("payload content for {method:?} file {i}").as_bytes())
                .unwrap();
        }

        let zip_bytes = writer.finish().unwrap().into_inner();
        let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();

        let dest = TempDir::with_prefix("par_extract_mem").unwrap();
        archive.par_extract(dest.path()).unwrap();

        for i in 0..25 {
            let expected_path = dest.path().join(format!("folder_{method:?}/file_{i}.txt"));
            assert!(expected_path.exists());
            let content = fs::read_to_string(&expected_path).unwrap();
            assert_eq!(content, format!("payload content for {method:?} file {i}"));
        }
    }
}

#[test]
fn test_par_extract_mixed_compression_methods() {
    let methods = enabled_read_write_compression_methods();
    let buf = Vec::new();
    let mut writer = ZipWriter::new(Cursor::new(buf));

    for i in 0..40 {
        let method = methods[i % methods.len()];
        let options = SimpleFileOptions::default().compression_method(method);
        let name = format!("mixed/entry_{i}.dat");
        writer.start_file(&name, options).unwrap();
        let payload = vec![(i % 251) as u8; 512 + (i * 13) % 1024];
        writer.write_all(&payload).unwrap();
    }

    let zip_bytes = writer.finish().unwrap().into_inner();
    let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();

    let dest = TempDir::with_prefix("par_extract_mixed").unwrap();
    archive.par_extract(dest.path()).unwrap();

    for i in 0..40 {
        let expected_path = dest.path().join(format!("mixed/entry_{i}.dat"));
        assert!(expected_path.exists());
        let expected_payload = vec![(i % 251) as u8; 512 + (i * 13) % 1024];
        let actual = fs::read(&expected_path).unwrap();
        assert_eq!(actual, expected_payload);
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

    let methods = enabled_read_write_compression_methods();
    for i in 0..40 {
        let folder = if i % 2 == 0 { "sub_alpha" } else { "sub_beta" };
        let name = format!("{folder}/data_{i}.bin");
        let method_options =
            SimpleFileOptions::default().compression_method(methods[i % methods.len()]);
        writer.start_file(&name, method_options).unwrap();
        let payload = vec![i as u8; 1024]; // 1 KiB each
        writer.write_all(&payload).unwrap();
    }

    let file = writer.finish().unwrap();
    drop(file);

    // Open file archive and extract in parallel
    let read_file = fs::File::open(&zip_path).unwrap();
    let mut archive = ZipArchive::new(read_file).unwrap();

    let dest = TempDir::with_prefix("par_extract_fs_dst").unwrap();
    archive
        .par_extract_from_path(&zip_path, dest.path())
        .unwrap();

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
        ZipError::DecompressedSizeLimitExceeded {
            size: 100,
            limit: 50
        }
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

#[test]
fn test_par_extract_zero_byte_files() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    for i in 0..15 {
        writer
            .start_file(format!("empty_{i}.dat"), options)
            .unwrap();
        // Zero bytes written
    }
    writer.start_file("non_empty.txt", options).unwrap();
    writer.write_all(b"not empty").unwrap();

    let zip_bytes = writer.finish().unwrap().into_inner();
    let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();

    let dest = TempDir::with_prefix("par_extract_zero").unwrap();
    archive.par_extract(dest.path()).unwrap();

    for i in 0..15 {
        let p = dest.path().join(format!("empty_{i}.dat"));
        assert!(p.exists());
        assert_eq!(fs::metadata(&p).unwrap().len(), 0);
    }
    let ne = dest.path().join("non_empty.txt");
    assert_eq!(fs::read(&ne).unwrap(), b"not empty");
}

#[test]
fn test_par_extract_deeply_nested_hierarchy() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let mut path = String::new();
    for depth in 1..=10 {
        path.push_str(&format!("lvl_{depth}/"));
        writer.add_directory(&path, options).unwrap();
        let file_path = format!("{path}file_at_depth_{depth}.txt");
        writer.start_file(&file_path, options).unwrap();
        writer
            .write_all(format!("depth {depth} payload").as_bytes())
            .unwrap();
    }

    let zip_bytes = writer.finish().unwrap().into_inner();
    let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();

    let dest = TempDir::with_prefix("par_extract_deep").unwrap();
    archive.par_extract(dest.path()).unwrap();

    let mut check_path = dest.path().to_path_buf();
    for depth in 1..=10 {
        check_path.push(format!("lvl_{depth}"));
        assert!(check_path.is_dir());
        let file_p = check_path.join(format!("file_at_depth_{depth}.txt"));
        assert!(file_p.is_file());
        assert_eq!(
            fs::read_to_string(&file_p).unwrap(),
            format!("depth {depth} payload")
        );
    }
}

#[test]
fn test_par_extract_high_concurrency_stress() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    for i in 0..100 {
        let name = format!("stress/worker_{i}.bin");
        writer.start_file(&name, options).unwrap();
        // Variable payloads from 0 up to 1024 bytes
        let size = (i * 17) % 1024;
        let payload = vec![i as u8; size];
        writer.write_all(&payload).unwrap();
    }

    let zip_bytes = writer.finish().unwrap().into_inner();
    let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();

    let dest = TempDir::with_prefix("par_extract_stress").unwrap();
    archive.par_extract(dest.path()).unwrap();

    for i in 0..100 {
        let p = dest.path().join(format!("stress/worker_{i}.bin"));
        assert!(p.exists());
        let expected_size = (i * 17) % 1024;
        let data = fs::read(&p).unwrap();
        assert_eq!(data.len(), expected_size);
        if expected_size > 0 {
            assert!(data.iter().all(|&b| b == (i as u8)));
        }
    }
}

#[test]
fn test_par_extract_preallocation_truncation_on_short_payload() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.start_file("parallel_short.bin", options).unwrap();
    writer.write_all(b"1234567890").unwrap(); // 10 bytes
    let mut zip_bytes = writer.finish().unwrap().into_inner();

    // Patch Central Directory record magic PK\x01\x02 (0x02014b50)
    let cd_magic = [0x50, 0x4b, 0x01, 0x02];
    let cd_pos = zip_bytes
        .windows(4)
        .position(|w| w == cd_magic)
        .expect("CD header magic must exist");

    // In CD header: offset 24..28 is uncompressed_size (u32 little endian)
    let fake_size: u32 = 256 * 1024; // 256 KiB
    zip_bytes[cd_pos + 24..cd_pos + 28].copy_from_slice(&fake_size.to_le_bytes());

    let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();
    assert_eq!(archive.by_index(0).unwrap().size(), 256 * 1024);

    let dest = TempDir::with_prefix("par_prealloc_trunc").unwrap();
    archive.par_extract(dest.path()).unwrap();

    let extracted_path = dest.path().join("parallel_short.bin");
    assert!(extracted_path.exists());
    let data = fs::read(&extracted_path).unwrap();
    assert_eq!(data, b"1234567890");
    // Verify file on disk was properly truncated back to 10 bytes, NOT 256 KiB
    assert_eq!(fs::metadata(&extracted_path).unwrap().len(), 10);
}

#[test]
fn test_par_extract_in_flight_size_limit_stream_exact_accounting() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    // 5 files, each 20 bytes = 100 bytes total
    for i in 0..5 {
        writer.start_file(format!("file_{i}.txt"), options).unwrap();
        writer.write_all(b"01234567890123456789").unwrap();
    }

    let zip_bytes = writer.finish().unwrap().into_inner();
    let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();

    let dest = TempDir::with_prefix("par_size_exact").unwrap();

    // Set size limit to 60 bytes (total 100 bytes)
    let res = archive
        .par_extract_with_options(dest.path(), ExtractOptions::default().with_size_limit(60));

    match res {
        Err(ZipError::DecompressedSizeLimitExceeded { size, limit }) => {
            assert_eq!(limit, 60);
            assert!(size >= 60, "Recorded size must reflect the limit breach");
        }
        other => panic!("Expected DecompressedSizeLimitExceeded, got {other:?}"),
    }
}
