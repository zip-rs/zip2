//! Only on little endian because we cannot use fs with miri CI
#![cfg(all(target_endian = "little", not(miri)))]

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
