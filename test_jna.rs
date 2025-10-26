use std::fs::File;
use std::io::BufReader;

fn main() {
    // First, let's try to download the file mentioned in the bug report
    let url = "https://repo1.maven.org/maven2/net/java/dev/jna/jna/5.6.0/jna-5.6.0.jar";
    
    println!("Downloading {}", url);
    
    let response = ureq::get(url).call().unwrap();
    let mut file = File::create("jna-5.6.0.jar").unwrap();
    std::io::copy(&mut response.into_reader(), &mut file).unwrap();
    
    println!("Downloaded jna-5.6.0.jar");
    
    // Now try to open it with the zip library
    let file = File::open("jna-5.6.0.jar").unwrap();
    let reader = BufReader::new(file);
    
    match zip::ZipArchive::new(reader) {
        Ok(archive) => {
            println!("Successfully opened archive with {} files", archive.len());
        }
        Err(e) => {
            println!("Error opening archive: {}", e);
        }
    }
}