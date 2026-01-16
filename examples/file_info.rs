use std::error::Error;
use std::fs;
use std::io::BufReader;
use zip::result::ZipError;
use zip::ZipArchive;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {:?} <filename>", args[0]);
        return Err("Wrong usage".into());
    }
    let fname_arg = &args[1];
    let fname_path = std::path::Path::new(fname_arg);
    // Validate and constrain the input path to a safe base directory to
    // avoid opening files outside the intended location.
    let safe_base = match std::env::current_dir() {
        Ok(dir) => dir.join("files"),
        Err(e) => {
            eprintln!("Error determining current directory: {e}");
            return 1;
        }
    };
    if let Err(e) = fs::create_dir_all(&safe_base) {
        eprintln!(
            "Error ensuring safe base directory exists {:?}: {e}",
            safe_base
        );
        return Err("Unsafe path".into());
    }
    let candidate_path = safe_base.join(fname_path);
    let safe_path = match candidate_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error resolving path {:?}: {e}", candidate_path);
            return 1;
        }
    };
    if !safe_path.starts_with(&safe_base) {
        eprintln!(
            "Error: refusing to open path outside of safe directory: {:?}",
            fname_arg
        );
        return 1;
    }
    let fname = safe_path;
    let mut archive = match fs::File::open(&fname)
        .map_err(ZipError::from)
        .and_then(|file| ZipArchive::new(BufReader::new(file)))
    {
        Ok(archive) => archive,
        Err(e) => {
            eprintln!("Error opening {:?}: {e}", fname.display());
            return 1;
        }
    };

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

    Ok(())
}
