use std::fs::{self, File};
use std::io::{self, Read, Write, Seek, SeekFrom};
use std::path::Path;
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

// This test verifies that we can append to a zip file with non-UTF-8 filenames
// The test creates a zip file with a non-UTF-8 filename manually, then appends to it
#[test]
fn test_append_to_zip_with_non_utf8_filename() -> io::Result<()> {
    // Create a temporary directory for our test
    let temp_dir = tempfile::tempdir()?;
    let zip_path = temp_dir.path().join("test.zip");
    
    // Create a zip file with a non-UTF-8 filename manually
    // This simulates a zip file created by 7-Zip with CP437 encoding
    create_zip_with_non_utf8_filename(&zip_path)?;
    
    // Verify the zip file is valid
    {
        let file = File::open(&zip_path)?;
        let mut archive = ZipArchive::new(file)?;
        
        assert_eq!(archive.len(), 1);
        
        // Get the first file name
        let file_name = archive.file_names().next().unwrap();
        println!("File name in archive: {}", file_name);
        
        // Read the content to verify it's correct
        let mut file = archive.by_index(0)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        assert_eq!(contents, "test content");
    }
    
    // Now append to the zip file
    {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&zip_path)?;
        
        let mut zip = ZipWriter::new_append(file)?;
        
        // Add a new file
        let options = SimpleFileOptions::default();
        zip.start_file("new_file.txt", options)?;
        zip.write_all(b"new content")?;
        
        zip.finish()?;
    }
    
    // Verify the zip file is still valid and contains both files
    {
        let file = File::open(&zip_path)?;
        let mut archive = ZipArchive::new(file)?;
        
        assert_eq!(archive.len(), 2);
        
        // Check the second file
        let mut file = archive.by_name("new_file.txt")?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        assert_eq!(contents, "new content");
    }
    
    Ok(())
}

// Create a zip file with a non-UTF-8 filename manually
fn create_zip_with_non_utf8_filename(path: &Path) -> io::Result<()> {
    let mut file = File::create(path)?;
    
    // Local file header
    file.write_all(b"PK\x03\x04")?; // Local file header signature
    file.write_all(&[20, 0])?;       // Version needed to extract
    file.write_all(&[0, 0])?;        // General purpose bit flag (0 = not UTF-8)
    file.write_all(&[0, 0])?;        // Compression method (0 = stored)
    file.write_all(&[0, 0])?;        // Last mod file time
    file.write_all(&[0, 0])?;        // Last mod file date
    file.write_all(&[0x35, 0x12, 0, 0])?; // CRC-32
    file.write_all(&[12, 0, 0, 0])?; // Compressed size
    file.write_all(&[12, 0, 0, 0])?; // Uncompressed size
    file.write_all(&[5, 0])?;        // File name length
    file.write_all(&[0, 0])?;        // Extra field length
    
    // File name (ó.txt with CP437 encoding)
    file.write_all(&[0xA2, b'.', b't', b'x', b't'])?;
    
    // File data
    file.write_all(b"test content")?;
    
    // Central directory header
    let central_dir_offset = file.stream_position()?;
    
    file.write_all(b"PK\x01\x02")?; // Central directory header signature
    file.write_all(&[20, 0])?;       // Version made by
    file.write_all(&[20, 0])?;       // Version needed to extract
    file.write_all(&[0, 0])?;        // General purpose bit flag (0 = not UTF-8)
    file.write_all(&[0, 0])?;        // Compression method
    file.write_all(&[0, 0])?;        // Last mod file time
    file.write_all(&[0, 0])?;        // Last mod file date
    file.write_all(&[0x35, 0x12, 0, 0])?; // CRC-32
    file.write_all(&[12, 0, 0, 0])?; // Compressed size
    file.write_all(&[12, 0, 0, 0])?; // Uncompressed size
    file.write_all(&[5, 0])?;        // File name length
    file.write_all(&[0, 0])?;        // Extra field length
    file.write_all(&[0, 0])?;        // File comment length
    file.write_all(&[0, 0])?;        // Disk number start
    file.write_all(&[0, 0])?;        // Internal file attributes
    file.write_all(&[0, 0, 0, 0])?;  // External file attributes
    file.write_all(&[0, 0, 0, 0])?;  // Relative offset of local header
    
    // File name (ó.txt with CP437 encoding)
    file.write_all(&[0xA2, b'.', b't', b'x', b't'])?;
    
    // End of central directory record
    let end_central_dir_offset = file.stream_position()?;
    
    file.write_all(b"PK\x05\x06")?; // End of central directory signature
    file.write_all(&[0, 0])?;        // Number of this disk
    file.write_all(&[0, 0])?;        // Disk where central directory starts
    file.write_all(&[1, 0])?;        // Number of central directory records on this disk
    file.write_all(&[1, 0])?;        // Total number of central directory records
    file.write_all(&[(end_central_dir_offset - central_dir_offset) as u8, 0, 0, 0])?; // Size of central directory
    file.write_all(&[central_dir_offset as u8, 0, 0, 0])?; // Offset of start of central directory
    file.write_all(&[0, 0])?;        // Comment length
    
    Ok(())
}