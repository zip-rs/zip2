use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use zip::read::root_dir_common_filter;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

fn create_zip_with_root_dir() -> Vec<u8> {
    let buf = Vec::new();
    let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
    let options = SimpleFileOptions::default();

    // Create a zip with a single top-level directory that contains everything else
    writer.add_directory("root/", options).unwrap();
    writer.add_directory("root/subdir/", options).unwrap();
    writer.start_file("root/file1.txt", options).unwrap();
    writer.write_all(b"File 1 content").unwrap();
    writer.start_file("root/subdir/file2.txt", options).unwrap();
    writer.write_all(b"File 2 content").unwrap();

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
    let options = SimpleFileOptions::default();

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
fn test_extract_without_root_dir() {
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
    fs::File::open(temp_dir.path().join("file1.txt"))
        .unwrap()
        .read_to_string(&mut content)
        .unwrap();
    assert_eq!(content, "File 1 content");
}

#[test]
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
fn test_extract_without_root_dir_with_ds_store() {
    let zip_data = create_zip_with_root_dir_and_ds_store();
    let mut archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();

    let temp_dir = TempDir::new().unwrap();
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
fn test_custom_root_dir_filter() {
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
