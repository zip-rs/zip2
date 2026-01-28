use std::io::Write;
use zip::result::ZipResult;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

#[test]
fn test_absolute_paths() -> ZipResult<()> {
    // Create a ZIP file with absolute paths
    let buf = Vec::new();
    let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
    let options = {
        #[cfg(all(feature = "deflate-zopfli", not(feature = "deflate-flate2")))]
        {
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)
        }
        #[cfg(not(all(feature = "deflate-zopfli", not(feature = "deflate-flate2"))))]
        SimpleFileOptions::default()
    };

    // Create entries with absolute paths
    writer.add_directory("/_/", options)?;
    writer.start_file("/_/test.txt", options)?;
    writer.write_all(b"Hello, World!")?;
    writer.start_file("/_/subdir/nested.txt", options)?;
    writer.write_all(b"Nested file content")?;

    let zip_data = writer.finish()?.into_inner();

    // Try to read the ZIP file
    let mut archive = ZipArchive::new(std::io::Cursor::new(zip_data))?;

    println!("ZIP file created with {} entries", archive.len());

    // Test individual file access
    assert_eq!(archive.len(), 3); // directory + 2 files

    for entry_index in 0..archive.len() {
        let file = archive.by_index(entry_index)?;

        // Verify that enclosed_name properly handles the paths
        let enclosed_name = file
            .enclosed_name()
            .expect("enclosed_name should not be None for valid paths");
        assert!(
            !enclosed_name.is_absolute(),
            "enclosed_name for '{}' should be relative, but was: {:?}",
            file.name(),
            enclosed_name
        );
    }

    // Try to extract the ZIP file
    let temp_dir = tempfile::TempDir::new()?;

    archive.extract(temp_dir.path())?;

    let base_path = temp_dir.path();
    assert!(base_path.join("_").is_dir());
    assert!(base_path.join("_/subdir").is_dir());

    let test_file_path = base_path.join("_/test.txt");
    assert!(test_file_path.is_file());
    assert_eq!(std::fs::read_to_string(test_file_path)?, "Hello, World!");

    let nested_file_path = base_path.join("_/subdir/nested.txt");
    assert!(nested_file_path.is_file());
    assert_eq!(
        std::fs::read_to_string(nested_file_path)?,
        "Nested file content"
    );

    Ok(())
}
