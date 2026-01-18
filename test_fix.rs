use std::io::Write;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

fn main() {
    println!("Testing the fix for absolute paths in ZIP files...");
    
    // Create a ZIP file with absolute paths
    let buf = Vec::new();
    let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
    let options = SimpleFileOptions::default();

    // Create entries with absolute paths
    writer.add_directory("/_/", options).unwrap();
    writer.start_file("/_/test.txt", options).unwrap();
    writer.write_all(b"Hello, World!").unwrap();
    writer.start_file("/_/subdir/nested.txt", options).unwrap();
    writer.write_all(b"Nested file content").unwrap();

    let zip_data = writer.finish().unwrap().into_inner();
    
    // Try to read the ZIP file
    let mut archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();
    
    println!("ZIP file created with {} entries", archive.len());
    
    // Test individual file access
    for i in 0..archive.len() {
        let file = archive.by_index(i).unwrap();
        println!("File {}: {}", i, file.name());
        
        let enclosed_name = file.enclosed_name();
        match enclosed_name {
            Some(path) => {
                println!("  Enclosed name: {:?}", path);
                println!("  Is absolute: {}", path.is_absolute());
            }
            None => {
                println!("  Enclosed name: None (invalid path)");
            }
        }
    }
    
    // Try to extract the ZIP file
    let temp_dir = tempfile::TempDir::new().unwrap();
    println!("Extracting to: {:?}", temp_dir.path());
    
    match archive.extract(temp_dir.path()) {
        Ok(_) => {
            println!("✅ Extraction successful!");
            
            // Check if files were extracted correctly
            let extracted_files = std::fs::read_dir(temp_dir.path()).unwrap();
            for entry in extracted_files {
                let entry = entry.unwrap();
                println!("  Extracted: {:?}", entry.path());
            }
            
            // Check specific files
            let test_file = temp_dir.path().join("_/test.txt");
            if test_file.exists() {
                let content = std::fs::read_to_string(&test_file).unwrap();
                println!("  Content of _/test.txt: {}", content);
            }
        }
        Err(e) => {
            println!("❌ Extraction failed: {}", e);
        }
    }
    
    println!("Test completed!");
}