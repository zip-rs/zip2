use std::io::Read;

fn main() {
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 2 {
        println!("Usage: {} <filename>", args[0]);
        return 1;
    }
    // Constrain the file path to the current directory and prevent directory traversal.
    let base_dir = std::path::Path::new(".");
    let requested_path = base_dir.join(&*args[1]);
    let fname = match std::fs::canonicalize(&requested_path) {
        Ok(p) => {
            if !p.starts_with(base_dir) {
                println!("Error: invalid filename (outside allowed directory)");
                return 1;
            }
            p
        }
        Err(e) => {
            println!("Error: cannot access {:?}: {e}", args[1]);
            return 1;
        }
    };
    let zipfile = match std::fs::File::open(&fname) {
        Ok(f) => f,
        Err(e) => {
            println!("Error: failed to open {:?}: {e}", fname.display());
            return 1;
        }
    };

    let mut archive = zip::ZipArchive::new(zipfile).unwrap();

    let mut file = match archive.by_name("test/lorem_ipsum.txt") {
        Ok(file) => file,
        Err(..) => {
            println!("File test/lorem_ipsum.txt not found");
            return 2;
        }
    };

    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();
    println!("{contents}");

    0
}
