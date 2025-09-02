// Minimal compilation test
fn main() {
    println!("Testing compilation...");
    
    // Just reference the compression module to check if it compiles
    let _method = zip::compression::CompressionMethod::LZMA;
    println!("LZMA compression method: {:?}", _method);
    
    println!("Compilation test passed!");
}