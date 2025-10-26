use std::io::Cursor;

fn main() {
    // Download the problematic JAR file
    let url = "https://repo1.maven.org/maven2/net/java/dev/jna/jna/5.6.0/jna-5.6.0.jar";
    
    println!("Downloading {}", url);
    let response = ureq::get(url).call().expect("Failed to download JAR file");
    let mut data = Vec::new();
    std::io::copy(&mut response.into_reader(), &mut data).expect("Failed to read response");
    
    println!("Downloaded {} bytes", data.len());
    
    // Try to open it with the zip library
    let cursor = Cursor::new(data);
    let result = zip::ZipArchive::new(cursor);
    
    match result {
        Ok(archive) => {
            println!("Successfully opened archive with {} files", archive.len());
        }
        Err(e) => {
            println!("Failed to open JAR file: {}", e);
        }
    }
}