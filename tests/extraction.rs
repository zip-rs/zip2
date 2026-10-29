//! Only on native because we cannot use fs with miri CI
#![cfg(not(miri))]

#[test]
fn test_can_create_destination_stream() {
    use tempfile::TempDir;
    use zip::unstable::stream::ZipStreamReader;

    let v = include_bytes!("../tests/data/mimetype.zip");
    let reader = ZipStreamReader::new(v.as_ref());
    let dest = TempDir::with_prefix("stream_test_can_create_destination").unwrap();
    reader.extract(&dest).unwrap();
    assert!(dest.path().join("mimetype").exists());
}

#[test]
fn test_extract_with_zip_stream() {
    use std::io::Cursor;
    use std::io::Write;
    use tempfile::TempDir;
    use zip::CompressionMethod;
    use zip::ZipWriter;
    use zip::{unstable::stream::ZipStreamReader, write::SimpleFileOptions};

    let buf = Vec::new();
    let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    // Create a ZIP with directory traversal attempts
    writer.start_file("file_test", options).unwrap();
    writer.write_all(b"content").unwrap();
    writer.add_directory("dir/", options).unwrap();
    writer
        .start_file("dir/file_test_in_folder", options)
        .unwrap();
    writer.write_all(b"content").unwrap();
    writer.add_directory("empty_dir/", options).unwrap();
    writer.add_symlink("symlink", "file_test", options).unwrap();

    let zip_data = writer.finish().unwrap().into_inner();
    let curr = Cursor::new(zip_data);

    let reader = ZipStreamReader::new(curr);
    let dest = TempDir::with_prefix("stream_test_can_create_destination").unwrap();
    reader.extract(&dest).unwrap();
    assert!(dest.path().join("file_test").exists());
    assert!(dest.path().join("file_test").is_file());
    assert!(dest.path().join("dir/").exists());
    assert!(dest.path().join("dir/").is_dir());
    assert!(dest.path().join("dir/file_test_in_folder").exists());
    assert!(dest.path().join("dir/file_test_in_folder").is_file());

    // Will be create because the path ends with `/`
    assert!(dest.path().join("empty_dir/").exists());
    assert!(dest.path().join("empty_dir/").is_dir());

    // Does not exists because we cannot know if it's a symlink
    // (no attributes in the local header)
    assert!(!dest.path().join("symlink/").exists());
    assert!(!dest.path().join("symlink/").is_dir());
}

/// Two entries that resolve to the same output path ("a" and "./a"), both with a unix mode:
/// extraction keeps the later one, as Info-ZIP does. The mode bookkeeping used to
/// `debug_assert` that each path was seen once, so debug builds panicked here.
#[test]
#[cfg(all(
    unix,
    not(all(feature = "deflate-zopfli", not(feature = "deflate-flate2")))
))]
fn extract_two_entries_with_the_same_path() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use zip::write::SimpleFileOptions;

    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.start_file("a", SimpleFileOptions::default().unix_permissions(0o600))
        .unwrap();
    w.write_all(b"one").unwrap();
    w.start_file("./a", SimpleFileOptions::default().unix_permissions(0o640))
        .unwrap();
    w.write_all(b"two").unwrap();
    let mut archive = zip::ZipArchive::new(w.finish().unwrap()).unwrap();

    let dest = tempfile::TempDir::with_prefix("extract_same_path").unwrap();
    archive.extract(dest.path()).unwrap();
    let path = dest.path().join("a");
    assert_eq!(std::fs::read(&path).unwrap(), b"two");
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn test_extract_with_preallocation_and_duplicate_dirs() {
    use std::io::{Cursor, Write};
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    let buf = Vec::new();
    let mut writer = ZipWriter::new(Cursor::new(buf));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    // Different raw entry names that resolve to the same directory
    writer.add_directory("sub/", options).unwrap();
    writer.add_directory("./sub/", options).unwrap();
    writer.start_file("sub/hello.txt", options).unwrap();
    writer.write_all(b"Hello Preallocation!").unwrap();

    let zip_bytes = writer.finish().unwrap().into_inner();
    let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();

    let dest = TempDir::with_prefix("prealloc_test").unwrap();
    archive.extract(dest.path()).unwrap();

    let file_path = dest.path().join("sub/hello.txt");
    assert!(file_path.exists());
    let data = std::fs::read(&file_path).unwrap();
    assert_eq!(data, b"Hello Preallocation!");
    assert_eq!(std::fs::metadata(&file_path).unwrap().len(), 20);
}

#[test]
fn test_preallocation_truncation_on_short_payload() {
    use std::io::{Cursor, Write};
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.start_file("short.bin", options).unwrap();
    writer.write_all(b"1234567890").unwrap(); // 10 bytes
    let mut zip_bytes = writer.finish().unwrap().into_inner();

    // Patch the Central Directory header for short.bin to claim 256 KiB uncompressed size
    // Search for Central Directory record magic PK\x01\x02 (0x02014b50)
    let cd_magic = [0x50, 0x4b, 0x01, 0x02];
    let cd_pos = zip_bytes
        .windows(4)
        .position(|w| w == cd_magic)
        .expect("CD header magic must exist");

    // In CD header: offset 24..28 is uncompressed_size (u32 little endian)
    let fake_size: u32 = 256 * 1024;
    zip_bytes[cd_pos + 24..cd_pos + 28].copy_from_slice(&fake_size.to_le_bytes());

    let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();
    assert_eq!(archive.by_index(0).unwrap().size(), 256 * 1024);

    let dest = TempDir::with_prefix("prealloc_trunc").unwrap();
    archive.extract(dest.path()).unwrap();

    let extracted_path = dest.path().join("short.bin");
    assert!(extracted_path.exists());
    let data = std::fs::read(&extracted_path).unwrap();
    assert_eq!(data, b"1234567890");
    // Verify file on disk was properly truncated back to 10 bytes, NOT 256 KiB
    assert_eq!(std::fs::metadata(&extracted_path).unwrap().len(), 10);
}
