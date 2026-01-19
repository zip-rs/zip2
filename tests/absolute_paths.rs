use std::io::Write;
use zip::result::ZipResult;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

#[test]
fn test_absolute_paths() -> ZipResult<()> {
    // Create a ZIP file with absolute paths
    let buf = Vec::new();
    let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
    let options = SimpleFileOptions::default();

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
    
    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        let enclosed_name = file.enclosed_name();
        
        // Verify that enclosed_name properly handles the paths
        match enclosed_name {
            Some(path) => {
                assert!(!path.is_absolute(), "Enclosed path should not be absolute: {:?}", path);
            }
            None => {
                // This might be expected for certain invalid paths
            }
        }
    }

    // Try to extract the ZIP file
    let temp_dir = tempfile::TempDir::new()?;
    
    archive.extract(temp_dir.path())?;
    
    // Verify extraction results with assertions
    let extracted_files: Vec<_> = std::fs::read_dir(temp_dir.path())?.collect();
    assert!(!extracted_files.is_empty(), "Should have extracted at least one file");
    
    // Check specific files exist and have correct content
    let test_file = temp_dir.path().join("test_dir/test.txt");
    assert!(test_file.exists(), "test.txt should be extracted");
    
    let content = std::fs::read_to_string(&test_file)?;
    assert_eq!(content, "Hello, World!", "File content should match expected value");
    Ok(())
}
