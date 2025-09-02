// Debug script to examine LZMA zip structure
use std::io::{self, Read, Seek, SeekFrom};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Examining LZMA zip file structure...");
    
    let data = include_bytes!("../data/lzma.zip");
    println!("File size: {} bytes", data.len());
    
    // Print first 100 bytes in hex
    println!("First 100 bytes:");
    for (i, chunk) in data.chunks(16).take(6).enumerate() {
        print!("{:04x}: ", i * 16);
        for byte in chunk {
            print!("{:02x} ", byte);
        }
        println!();
    }
    
    // Try to open with zip library to see what happens
    let mut archive = zip::ZipArchive::new(io::Cursor::new(data))?;
    println!("Archive opened successfully");
    println!("Number of files: {}", archive.len());
    
    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        println!("File {}: {} (method: {:?}, compressed: {}, uncompressed: {})", 
                 i, file.name(), file.compression(), file.compressed_size(), file.size());
    }
    
    Ok(())
}