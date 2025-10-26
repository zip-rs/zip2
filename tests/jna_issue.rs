use std::fs::File;
use std::io::{BufReader, Cursor};
use zip::ZipArchive;

#[test]
fn test_jna_jar_issue() {
    // Download the problematic JAR file
    let url = "https://repo1.maven.org/maven2/net/java/dev/jna/jna/5.6.0/jna-5.6.0.jar";
    
    let response = ureq::get(url).call().expect("Failed to download JAR file");
    let mut data = Vec::new();
    std::io::copy(&mut response.into_reader(), &mut data).expect("Failed to read response");
    
    // Try to open it with the zip library
    let cursor = Cursor::new(data);
    let result = ZipArchive::new(cursor);
    
    match result {
        Ok(archive) => {
            println!("Successfully opened archive with {} files", archive.len());
            assert_eq!(archive.len(), 170); // Expected number of files as mentioned in the bug report
        }
        Err(e) => {
            panic!("Failed to open JAR file: {}", e);
        }
    }
}