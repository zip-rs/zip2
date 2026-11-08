use std::io::Write;
#[cfg(not(miri))]
use std::path::Path;
use std::path::PathBuf;
#[cfg(not(miri))]
use tempfile::TempDir;
use zip::read::root_dir_common_filter;
#[cfg(not(miri))]
use zip::read::{ExtractOptions, SymlinkExtractAction};
#[cfg(not(miri))]
use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

fn readable_options() -> SimpleFileOptions {
    let options = SimpleFileOptions::default();
    // With zopfli but no flate2, Deflate is write-only, so the default method can't be read back.
    #[cfg(all(feature = "deflate-zopfli", not(feature = "deflate-flate2")))]
    let options = options.compression_method(zip::CompressionMethod::Stored);
    options
}

fn create_zip_with_root_dir() -> Vec<u8> {
    let buf = Vec::new();
    let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
    let options = readable_options();

    // Create a zip with a single top-level directory that contains everything else
    writer.add_directory("root/", options).unwrap();
    writer.add_directory("root/subdir/", options).unwrap();
    writer.start_file("root/file1.txt", options).unwrap();
    writer.write_all(b"File 1 content").unwrap();
    writer.start_file("root/subdir/file2.txt", options).unwrap();
    writer.write_all(b"File 2 content").unwrap();

    writer.finish().unwrap().into_inner()
}

#[cfg(not(miri))]
fn create_zip_with_symlink_root_dir() -> Vec<u8> {
    let buf = Vec::new();
    let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
    let options = readable_options();

    // Create a zip with a single top-level directory that also contains a symlink
    writer.add_directory("root/", options).unwrap();
    writer.add_directory("root/subdir/", options).unwrap();
    writer.start_file("root/file1.txt", options).unwrap();
    writer.write_all(b"File 1 content").unwrap();
    writer.start_file("root/subdir/file2.txt", options).unwrap();
    writer.write_all(b"File 2 content").unwrap();
    writer
        .add_symlink("root/link", "file1.txt", options)
        .unwrap();

    writer.finish().unwrap().into_inner()
}

fn create_zip_without_root_dir() -> Vec<u8> {
    let buf = Vec::new();
    let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
    let options = SimpleFileOptions::default();

    // Create a zip with multiple top-level entries
    writer.add_directory("dir1/", options).unwrap();
    writer.add_directory("dir2/", options).unwrap();
    writer.start_file("file1.txt", options).unwrap();
    writer.write_all(b"File 1 content").unwrap();
    writer.start_file("dir1/file2.txt", options).unwrap();
    writer.write_all(b"File 2 content").unwrap();

    writer.finish().unwrap().into_inner()
}

fn create_zip_with_root_dir_and_ds_store() -> Vec<u8> {
    let buf = Vec::new();
    let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
    let options = readable_options();

    // Create a zip with a single top-level directory and a .DS_Store file
    writer.start_file(".DS_Store", options).unwrap();
    writer.write_all(b"DS_Store content").unwrap();
    writer.add_directory("root/", options).unwrap();
    writer.add_directory("root/subdir/", options).unwrap();
    writer.start_file("root/file1.txt", options).unwrap();
    writer.write_all(b"File 1 content").unwrap();
    writer.start_file("root/subdir/file2.txt", options).unwrap();
    writer.write_all(b"File 2 content").unwrap();

    writer.finish().unwrap().into_inner()
}

fn create_zip_with_macosx_dir() -> Vec<u8> {
    let buf = Vec::new();
    let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
    let options = SimpleFileOptions::default();

    // Create a zip with __MACOSX directory alongside the root dir
    writer.add_directory("__MACOSX/", options).unwrap();
    writer.start_file("__MACOSX/._file1.txt", options).unwrap();
    writer.write_all(b"Resource fork").unwrap();

    writer.add_directory("root/", options).unwrap();
    writer.start_file("root/file1.txt", options).unwrap();
    writer.write_all(b"File 1 content").unwrap();

    writer.finish().unwrap().into_inner()
}

fn create_zip_with_multiple_root_dirs() -> Vec<u8> {
    let buf = Vec::new();
    let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
    let options = SimpleFileOptions::default();

    // Create a zip with multiple top-level directories
    writer.add_directory("root1/", options).unwrap();
    writer.start_file("root1/file1.txt", options).unwrap();
    writer.write_all(b"File 1 content").unwrap();

    writer.add_directory("root2/", options).unwrap();
    writer.start_file("root2/file2.txt", options).unwrap();
    writer.write_all(b"File 2 content").unwrap();

    writer.finish().unwrap().into_inner()
}

#[test]
fn test_root_dir_with_single_root() {
    let zip_data = create_zip_with_root_dir();
    let archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();

    // Use the default common filter
    let root_dir = archive.root_dir(root_dir_common_filter).unwrap();

    assert!(root_dir.is_some());
    assert_eq!(root_dir.unwrap(), PathBuf::from("root"));
}

#[test]
fn test_root_dir_without_root() {
    let zip_data = create_zip_without_root_dir();
    let archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();

    let root_dir = archive.root_dir(root_dir_common_filter).unwrap();

    assert!(root_dir.is_none());
}

#[test]
fn test_root_dir_with_ds_store() {
    let zip_data = create_zip_with_root_dir_and_ds_store();
    let archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();

    // Without filtering, it should return None because .DS_Store is at the root level
    let root_dir = archive.root_dir(|_| true).unwrap();
    assert!(root_dir.is_none());

    // With common filter, it should ignore .DS_Store and find the root directory
    let root_dir = archive.root_dir(root_dir_common_filter).unwrap();
    assert!(root_dir.is_some());
    assert_eq!(root_dir.unwrap(), PathBuf::from("root"));
}

#[test]
fn test_root_dir_with_macosx_dir() {
    let zip_data = create_zip_with_macosx_dir();
    let archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();

    // Without filtering, it should return None because __MACOSX is at the root level
    let root_dir = archive.root_dir(|_| true).unwrap();
    assert!(root_dir.is_none());

    // With common filter, it should ignore __MACOSX and find the root directory
    let root_dir = archive.root_dir(root_dir_common_filter).unwrap();
    assert!(root_dir.is_some());
    assert_eq!(root_dir.unwrap(), PathBuf::from("root"));
}

#[test]
fn test_root_dir_with_multiple_root_dirs() {
    let zip_data = create_zip_with_multiple_root_dirs();
    let archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();

    // Should return None because there are multiple top-level directories
    let root_dir = archive.root_dir(root_dir_common_filter).unwrap();
    assert!(root_dir.is_none());
}

#[test]
#[cfg(all(not(miri), feature = "deflate-flate2"))]
fn test_extract_without_root_dir() {
    use std::io::Read;
    let zip_data = create_zip_with_root_dir();
    let mut archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();

    let temp_dir = TempDir::new().unwrap();
    archive
        .extract_unwrapped_root_dir(temp_dir.path(), root_dir_common_filter)
        .unwrap();

    // Files should be extracted directly without the root directory
    assert!(temp_dir.path().join("file1.txt").exists());
    assert!(temp_dir.path().join("subdir").exists());
    assert!(temp_dir.path().join("subdir/file2.txt").exists());

    // The root directory should not exist
    assert!(!temp_dir.path().join("root").exists());

    // Check file contents
    let mut content = String::new();
    std::fs::File::open(temp_dir.path().join("file1.txt"))
        .unwrap()
        .read_to_string(&mut content)
        .unwrap();
    assert_eq!(content, "File 1 content");
}

#[test]
#[cfg(all(not(miri), feature = "deflate-flate2"))]
fn test_extract_without_root_dir_but_no_root_found() {
    let zip_data = create_zip_without_root_dir();
    let mut archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();

    let temp_dir = TempDir::new().unwrap();
    archive
        .extract_unwrapped_root_dir(temp_dir.path(), root_dir_common_filter)
        .unwrap();

    // All files should be extracted normally since there's no single root directory
    assert!(temp_dir.path().join("file1.txt").exists());
    assert!(temp_dir.path().join("dir1").exists());
    assert!(temp_dir.path().join("dir2").exists());
    assert!(temp_dir.path().join("dir1/file2.txt").exists());
}

#[test]
#[cfg(all(not(miri), feature = "deflate-flate2"))]
fn test_extract_without_root_dir_with_ds_store() {
    let zip_data = create_zip_with_root_dir_and_ds_store();
    let mut archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();

    let temp_dir = tempfile::TempDir::new().unwrap();
    archive
        .extract_unwrapped_root_dir(temp_dir.path(), root_dir_common_filter)
        .unwrap();

    // .DS_Store should be ignored when finding the root dir
    assert!(temp_dir.path().join("file1.txt").exists());
    assert!(temp_dir.path().join("subdir").exists());
    assert!(temp_dir.path().join("subdir/file2.txt").exists());

    // The root directory should not exist
    assert!(!temp_dir.path().join("root").exists());

    // .DS_Store should still be extracted at the root level though
    assert!(temp_dir.path().join(".DS_Store").exists());
}

#[test]
#[cfg(not(miri))]
fn test_extract_with_options_unwraps_root_dir() {
    let mut archive = ZipArchive::new(std::io::Cursor::new(create_zip_with_root_dir())).unwrap();
    let temp_dir = TempDir::new().unwrap();

    archive
        .extract_with_options(
            temp_dir.path(),
            ExtractOptions::default().with_root_dir_filter(root_dir_common_filter),
        )
        .unwrap();

    assert!(temp_dir.path().join("file1.txt").exists());
    assert!(temp_dir.path().join("subdir/file2.txt").exists());
    assert!(!temp_dir.path().join("root").exists());
}

#[test]
#[cfg(not(miri))]
fn test_extract_with_options_follows_size_limit() {
    let mut archive = ZipArchive::new(std::io::Cursor::new(create_zip_with_root_dir())).unwrap();
    let temp_dir = TempDir::new().unwrap();

    let err = archive
        .extract_with_options(
            temp_dir.path(),
            ExtractOptions::default()
                .with_root_dir_filter(root_dir_common_filter)
                .with_size_limit(10),
        )
        .expect_err("size limit must still apply when unwrapping");
    assert!(matches!(
        err,
        ZipError::DecompressedSizeLimitExceeded {
            size: 28,
            limit: 10
        }
    ));
    assert!(!temp_dir.path().join("file1.txt").exists());
}

#[test]
#[cfg(not(miri))]
fn test_extract_with_root_dir_filter_carries_fields() {
    let mut archive = ZipArchive::new(std::io::Cursor::new(create_zip_with_root_dir())).unwrap();
    let temp_dir = TempDir::new().unwrap();

    let err = archive
        .extract_with_options(
            temp_dir.path(),
            ExtractOptions::default()
                .with_size_limit(10)
                .with_root_dir_filter(root_dir_common_filter),
        )
        .expect_err("size limit must still apply when unwrapping");
    assert!(matches!(
        err,
        ZipError::DecompressedSizeLimitExceeded {
            size: 28,
            limit: 10
        }
    ));
    assert!(!temp_dir.path().join("file1.txt").exists());
}

#[test]
#[cfg(not(miri))]
fn test_extract_with_symlink_action() {
    let mut archive =
        ZipArchive::new(std::io::Cursor::new(create_zip_with_symlink_root_dir())).unwrap();
    let temp_dir = TempDir::new().unwrap();

    archive
        .extract_with_options(
            temp_dir.path(),
            ExtractOptions::default()
                .with_root_dir_filter(root_dir_common_filter)
                .symlink_action(SymlinkExtractAction::NoExtract),
        )
        .unwrap();

    // The root directory was unwrapped...
    assert!(temp_dir.path().join("file1.txt").exists());
    assert!(!temp_dir.path().join("root").exists());
    // ...and the symlink was skipped. `exists()` follows symlinks, so check the link itself.
    assert!(std::fs::symlink_metadata(temp_dir.path().join("link")).is_err());
}

#[test]
#[cfg(not(miri))]
fn test_extract_with_options_capturing_filter() {
    let ignored = String::from(".DS_Store");
    let filter = move |path: &Path| path != Path::new(&ignored);

    let mut archive =
        ZipArchive::new(std::io::Cursor::new(create_zip_with_root_dir_and_ds_store())).unwrap();
    let temp_dir = TempDir::new().unwrap();

    archive
        .extract_with_options(
            temp_dir.path(),
            ExtractOptions::default().with_root_dir_filter(filter),
        )
        .unwrap();

    assert!(temp_dir.path().join("file1.txt").exists());
    assert!(!temp_dir.path().join("root").exists());
    // Filtered entries only affect root-dir detection; they're still extracted
    assert!(temp_dir.path().join(".DS_Store").exists());
}

#[test]
#[cfg(all(not(miri), feature = "rayon"))]
fn test_par_extract_explicit_filter_takes_precedence() {
    let data = create_zip_with_root_dir_and_ds_store();
    let mut archive = ZipArchive::new(std::io::Cursor::new(data.clone())).unwrap();
    let temp_dir = TempDir::new().unwrap();

    archive
        .par_extract_with_reader_factory_and_options(
            temp_dir.path(),
            || Ok(std::io::Cursor::new(data.clone())),
            // Would ignore .DS_Store, find `root/` and unwrap it...
            ExtractOptions::default().with_root_dir_filter(root_dir_common_filter),
            // ...but this one counts .DS_Store, so there's no single root
            Some(|_: &Path| true),
        )
        .unwrap();

    assert!(temp_dir.path().join("root/file1.txt").exists());
    assert!(!temp_dir.path().join("file1.txt").exists());
}

#[test]
#[cfg(all(not(miri), feature = "rayon"))]
fn test_par_extract_falls_back_to_options_filter() {
    let data = create_zip_with_root_dir_and_ds_store();
    let mut archive = ZipArchive::new(std::io::Cursor::new(data.clone())).unwrap();
    let temp_dir = TempDir::new().unwrap();

    archive
        .par_extract_with_reader_factory_and_options(
            temp_dir.path(),
            || Ok(std::io::Cursor::new(data.clone())),
            ExtractOptions::default().with_root_dir_filter(root_dir_common_filter),
            None::<fn(&Path) -> bool>,
        )
        .unwrap();

    assert!(temp_dir.path().join("file1.txt").exists());
    assert!(!temp_dir.path().join("root").exists());
}

#[test]
#[cfg(all(not(miri), feature = "rayon"))]
fn test_par_extract_with_options_unwraps_root_dir() {
    let mut archive = ZipArchive::new(std::io::Cursor::new(create_zip_with_root_dir())).unwrap();
    let temp_dir = TempDir::new().unwrap();

    archive
        .par_extract_with_options(
            temp_dir.path(),
            ExtractOptions::default().with_root_dir_filter(root_dir_common_filter),
        )
        .unwrap();

    assert!(temp_dir.path().join("file1.txt").exists());
    assert!(temp_dir.path().join("subdir/file2.txt").exists());
    assert!(!temp_dir.path().join("root").exists());
}

#[test]
#[cfg(all(not(miri), feature = "deflate-flate2"))]
fn test_custom_root_dir_filter() {
    use std::path::Path;
    let zip_data = create_zip_with_root_dir();
    let mut archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();

    // Define a custom filter that ignores files with "file1" in their name
    let custom_filter = |path: &Path| !path.to_string_lossy().contains("file1");

    let root_dir = archive.root_dir(custom_filter).unwrap();

    // Should still find the root directory even with our custom filter
    assert!(root_dir.is_some());
    assert_eq!(root_dir.unwrap(), PathBuf::from("root"));

    // Extract with our custom filter
    let temp_dir = TempDir::new().unwrap();
    archive
        .extract_unwrapped_root_dir(temp_dir.path(), custom_filter)
        .unwrap();

    // file1.txt should be skipped during root directory detection
    // but still extracted normally
    assert!(temp_dir.path().join("file1.txt").exists());
    assert!(temp_dir.path().join("subdir").exists());
    assert!(temp_dir.path().join("subdir/file2.txt").exists());
}
