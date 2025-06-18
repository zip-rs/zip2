use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

#[test]
fn test_append_to_zip_with_non_utf8_filename() -> io::Result<()> {
    // Create a temporary directory for our test
    let temp_dir = tempfile::tempdir()?;
    let zip_path = temp_dir.path().join("test.zip");
    
    // Create a zip file with a non-UTF-8 filename
    {
        let file = File::create(&zip_path)?;
        let mut zip = ZipWriter::new(file);
        
        // Create a file with a non-UTF-8 filename (CP437 encoded)
        // This is equivalent to "ó.txt" with 0xa2 (not 0xc3 0xb3)
        let filename_raw = vec![0xa2, b'.', b't', b'x', b't'];
        let options = SimpleFileOptions::default();
        
        // We need to use a raw API to create a file with a non-UTF-8 filename
        // For testing purposes, we'll create the file manually
        zip.start_file_from_raw_parts(filename_raw, options)?;
        zip.write_all(b"test content")?;
        
        zip.finish()?;
    }
    
    // Verify the zip file is valid and contains our file
    {
        let file = File::open(&zip_path)?;
        let mut archive = ZipArchive::new(file)?;
        
        assert_eq!(archive.len(), 1);
        
        // The filename should be decoded from CP437
        let file_name = archive.file_names().next().unwrap();
        assert_eq!(file_name, "ó.txt");
        
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
        
        // Check the first file (non-UTF-8 filename)
        let mut file = archive.by_name("ó.txt")?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        assert_eq!(contents, "test content");
        
        // Check the second file
        let mut file = archive.by_name("new_file.txt")?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        assert_eq!(contents, "new content");
    }
    
    Ok(())
}

trait ZipWriterExt {
    fn start_file_from_raw_parts<B: Into<Box<[u8]>>>(
        &mut self,
        name_raw: B,
        options: SimpleFileOptions,
    ) -> zip::result::ZipResult<()>;
}

impl<W: Write + io::Seek> ZipWriterExt for ZipWriter<W> {
    fn start_file_from_raw_parts<B: Into<Box<[u8]>>>(
        &mut self,
        name_raw: B,
        options: SimpleFileOptions,
    ) -> zip::result::ZipResult<()> {
        // This is a simplified version just for testing
        // It creates a file entry with the raw filename bytes
        let name_raw = name_raw.into();
        let name = String::from_utf8_lossy(&name_raw).into_owned();
        
        // Use the public API but then modify the internal data
        self.start_file(&name, options)?;
        
        // Access the last file entry and replace the filename_raw
        if let Some((_name, file_data)) = self.files.last_mut() {
            file_data.file_name_raw = name_raw;
            file_data.is_utf8 = false; // Mark as non-UTF-8
        }
        
        Ok(())
    }
}