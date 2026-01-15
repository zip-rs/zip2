use std::fs;
use std::io::BufReader;

fn main() {
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 2 {
        println!("Usage: {} <filename>", args[0]);
        return 1;
    }
    let fname_arg = &args[1];
    // Determine a trusted base directory (current working directory).
    let base_dir = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("Error: could not determine current directory: {e}");
            return 1;
        }
    };
    // Construct the path relative to the trusted base directory and canonicalize it.
    let candidate_path = base_dir.join(fname_arg);
    let fname = match candidate_path.canonicalize() {
        Ok(path) => {
            if !path.starts_with(&base_dir.canonicalize().unwrap_or(base_dir.clone())) {
                println!("Error: refusing to open path outside of base directory: {:?}", fname_arg);
                return 1;
            }
            path
        }
        Err(e) => {
            eprintln!("Error: could not open {:?}: {e}", fname_arg);
            return 1;
        }
    };
    let reader = BufReader::new(file);

    let mut archive = zip::ZipArchive::new(reader).unwrap();

    for i in 0..archive.len() {
        let file = archive.by_index(i).unwrap();
        let outpath = match file.enclosed_name() {
            Some(path) => path,
            None => {
                println!("Entry {} has a suspicious path", file.name());
                continue;
            }
        };

        {
            let comment = file.comment();
            if !comment.is_empty() {
                println!("Entry {i} comment: {comment}");
            }
        }

        if file.is_dir() {
            println!(
                "Entry {} is a directory with name \"{}\"",
                i,
                outpath.display()
            );
        } else {
            println!(
                "Entry {} is a file with name \"{}\" ({} bytes)",
                i,
                outpath.display(),
                file.size()
            );
        }
    }

    0
}
