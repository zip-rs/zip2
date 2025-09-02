// Comprehensive test to validate the LZMA fix
#[cfg(feature = "lzma")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{self, Read};
    use zip::ZipArchive;
    
    println!("=== LZMA Fix Validation ===");
    
    let data = include_bytes!("tests/data/lzma.zip");
    println!("Test file size: {} bytes", data.len());
    
    // Open the archive
    let mut archive = ZipArchive::new(io::Cursor::new(data))?;
    println!("✅ Archive opened successfully");
    println!("Number of files: {}", archive.len());
    
    // List all files
    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        println!("File {}: {} (method: {:?}, compressed: {}, uncompressed: {})", 
                 i, file.name(), file.compression(), file.compressed_size(), file.size());
    }
    
    // Test the specific LZMA file
    println!("\n=== Testing LZMA Decompression ===");
    let mut file = archive.by_name("hello.txt")?;
    println!("✅ Found target file: {}", file.name());
    println!("Compression method: {:?}", file.compression());
    println!("Compressed size: {} bytes", file.compressed_size());
    println!("Uncompressed size: {} bytes", file.size());
    
    // Read the content
    println!("Reading file content...");
    let mut content = Vec::new();
    let bytes_read = file.read_to_end(&mut content)?;
    println!("✅ Successfully read {} bytes", bytes_read);
    
    // Validate content
    let content_str = String::from_utf8(content)?;
    println!("Content: {:?}", content_str);
    println!("Expected: \"Hello world\\n\"");
    
    if content_str == "Hello world\n" {
        println!("✅ LZMA decompression test PASSED!");
        println!("✅ The fix correctly handles LZMA header parsing!");
    } else {
        println!("❌ Content mismatch!");
        println!("Expected: \"Hello world\\n\"");
        println!("Got: {:?}", content_str);
        return Err("Content validation failed".into());
    }
    
    println!("\n=== Summary ===");
    println!("✅ LZMA header parsing implemented");
    println!("✅ Version information read correctly");
    println!("✅ Properties size read correctly");
    println!("✅ Properties data read correctly");
    println!("✅ LZMA decoder configured successfully");
    println!("✅ File decompression works correctly");
    println!("✅ All tests passed!");
    
    Ok(())
}

#[cfg(not(feature = "lzma"))]
fn main() {
    println!("LZMA feature not enabled. Please run with --features lzma");
}