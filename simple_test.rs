// Simple test to check if the LZMA implementation works
#[cfg(feature = "lzma")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{self, Read};
    use zip::ZipArchive;
    
    println!("Testing LZMA decompression...");
    
    let data = include_bytes!("tests/data/lzma.zip");
    let mut archive = ZipArchive::new(io::Cursor::new(data))?;
    
    println!("Archive opened successfully");
    println!("Number of files: {}", archive.len());
    
    // Try to read the LZMA compressed file
    match archive.by_name("hello.txt") {
        Ok(mut file) => {
            println!("Found file: {}", file.name());
            println!("Compression method: {:?}", file.compression());
            
            let mut content = Vec::new();
            match file.read_to_end(&mut content) {
                Ok(_) => {
                    let content_str = String::from_utf8_lossy(&content);
                    println!("Successfully read content: {:?}", content_str);
                    
                    if content_str == "Hello world\n" {
                        println!("✅ LZMA test PASSED!");
                    } else {
                        println!("❌ Content mismatch. Expected: \"Hello world\\n\", Got: {:?}", content_str);
                    }
                }
                Err(e) => {
                    println!("❌ Failed to read file content: {}", e);
                    return Err(e.into());
                }
            }
        }
        Err(e) => {
            println!("❌ Failed to find file: {}", e);
            return Err(e.into());
        }
    }
    
    Ok(())
}

#[cfg(not(feature = "lzma"))]
fn main() {
    println!("LZMA feature not enabled");
}