use std::fs;
use std::io;

fn main() {
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 2 {
        println!("Usage: {} <filename>", args[0]);
        return 1;
    }

    let file_arg = &args[1];
    if file_arg.contains("..") || file_arg.contains('/') || file_arg.contains('\\') {
        eprintln!(
            "Error: invalid filename '{}'. Directory separators and \"..\" are not allowed.",
            file_arg
        );
        return 1;
    }

    // Build a path to the archive relative to a safe base directory (current working directory)
    let base_dir = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("Error: unable to determine current directory: {e}");
            return 1;
        }
    };

    let candidate_path = base_dir.join(std::path::Path::new(file_arg));
    let fname = match candidate_path.canonicalize() {
        Ok(path) => {
            if !path.starts_with(&base_dir) {
                eprintln!(
                    "Error: resolved path '{}' escapes the allowed directory.",
                    path.display()
                );
                return 1;
            }
            path
        }
        Err(e) => {
            eprintln!(
                "Error: unable to resolve archive path '{}': {e}",
                candidate_path.display()
            );
            return 1;
        }
    };

    let file = fs::File::open(&fname).unwrap();

    let mut archive = zip::ZipArchive::new(file).unwrap();

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).unwrap();
        let outpath = match file.enclosed_name() {
            Some(path) => path,
            None => continue,
        };

        {
            let comment = file.comment();
            if !comment.is_empty() {
                println!("File {i} comment: {comment}");
            }
        }

        if file.is_dir() {
            println!("File {} extracted to \"{}\"", i, outpath.display());
            fs::create_dir_all(&outpath).unwrap();
        } else {
            println!(
                "File {} extracted to \"{}\" ({} bytes)",
                i,
                outpath.display(),
                file.size()
            );
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    fs::create_dir_all(p).unwrap();
                }
            }
            let mut outfile = fs::File::create(&outpath).unwrap();
            io::copy(&mut file, &mut outfile).unwrap();
        }

        // Get and Set permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            if let Some(mode) = file.unix_mode() {
                fs::set_permissions(&outpath, fs::Permissions::from_mode(mode)).unwrap();
            }
        }
    }

    0
}
