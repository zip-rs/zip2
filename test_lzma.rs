// Simple test to check LZMA implementation
use std::io::{self, Read};
use zip::ZipArchive;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing LZMA implementation...");
    
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("tests/data/lzma.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v))?;

    let mut file = archive.by_name("hello.txt")?;
    println!("Found file: {}", file.name());

    let mut content = Vec::new();
    file.read_to_end(&mut content)?;
    let content_str = String::from_utf8(content)?;
    
    println!("Content: {:?}", content_str);
    println!("Expected: \"Hello world\\n\"");
    
    if content_str == "Hello world\n" {
        println!("✅ LZMA test passed!");
    } else {
        println!("❌ LZMA test failed!");
    }
    
    Ok(())
}