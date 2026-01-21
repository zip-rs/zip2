use std::{error::Error, io::Read};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {:?} <filename>", args[0]);
        return Err("Wrong usage".into());
    }
    // Constrain the file path to the current directory and prevent directory traversal.
    let base_dir = std::path::Path::new(".");
    let requested_path = base_dir.join(&*args[1]);
    let fname = match std::fs::canonicalize(&requested_path) {
        Ok(p) => {
            if !p.starts_with(base_dir) {
                eprintln!("Error: invalid filename (outside allowed directory)");
                return Err("Invalid filename".into());
            }
            p
        }
        Err(e) => {
            println!("Error: cannot access {:?}: {e}", args[1]);
            return Err(e.into());
        }
    };
    let zipfile = match std::fs::File::open(&fname) {
        Ok(f) => f,
        Err(e) => {
            println!("Error: failed to open {:?}: {e}", fname.display());
            return Err(e.into());
        }
    };

    let mut archive = zip::ZipArchive::new(zipfile).unwrap();

    let mut file = match archive.by_name("test/lorem_ipsum.txt") {
        Ok(file) => file,
        Err(e) => {
            println!("File test/lorem_ipsum.txt not found");
            return Err(e.into());
        }
    };

    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();
    println!("{contents}");

    Ok(())
}
