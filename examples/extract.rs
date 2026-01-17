use std::fs;
use std::io;
use zip::result::ZipError;
use zip::ZipArchive;

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

    // Validate the path without requiring the file to exist
    let out_root = candidate_path.components().collect::<std::path::PathBuf>();
    if !out_root.starts_with(&base_dir) {
        eprintln!(
            "Error: path '{}' escapes the allowed directory.",
            candidate_path.display()
        );
        return 1;
    }

    let mut archive = match fs::File::open(&out_root)
        .map_err(ZipError::from)
        .and_then(ZipArchive::new)
    {
        Ok(archive) => archive,
        Err(e) => {
            eprintln!(
                "Error: unable to open archive {:?}: {e}",
                out_root.display()
            );
            return 1;
        }
    };

    let mut some_files_failed = false;
    for i in 0..archive.len() {
        let mut file = match archive.by_index(i) {
            Ok(file) => file,
            Err(e) => {
                eprintln!("Error: unable to open file {i} in archive: {e}");
                some_files_failed = true;
                continue;
            }
        };
        let out_path = match file.enclosed_name() {
            Some(path) => path,
            None => {
                eprintln!(
                    "Error: unable to extract file {} because it has an invalid path.",
                    file.name()
                );
                some_files_failed = true;
                continue;
            }
        };
        let comment = file.comment();
        if !comment.is_empty() {
            println!("File {i} comment: {comment}");
        }
        if file.is_dir() {
            if let Err(e) = fs::create_dir_all(&out_path) {
                eprintln!(
                    "Error: unable to extract directory {i} to {:?}: {e}",
                    out_path.display()
                );
                some_files_failed = true;
                continue;
            } else {
                println!("Directory {i} extracted to {:?}", out_path.display());
            }
        } else {
            if let Some(p) = out_path.parent() {
                if !p.exists() {
                    if let Err(e) = fs::create_dir_all(p) {
                        eprintln!(
                            "Error: unable to create parent directory {p:?} of file {}: {e}",
                            p.display()
                        );
                        some_files_failed = true;
                        continue;
                    }
                }
            }
            match fs::File::create(&out_path)
                .and_then(|mut outfile| io::copy(&mut file, &mut outfile))
            {
                Ok(bytes_extracted) => {
                    println!(
                        "File {} extracted to {:?} ({bytes_extracted} bytes)",
                        i,
                        out_path.display(),
                    );
                }
                Err(e) => {
                    eprintln!(
                        "Error: unable to extract file {i} to {:?}: {e}",
                        out_path.display()
                    );
                    some_files_failed = true;
                    continue;
                }
            }
        }

        // Get and Set permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            if let Some(mode) = file.unix_mode() {
                if let Err(e) = fs::set_permissions(&out_path, fs::Permissions::from_mode(mode)) {
                    eprintln!(
                        "Error: unable to change permissions of file {i} ({:?}): {e}",
                        out_path.display()
                    );
                    some_files_failed = true;
                }
            }
        }
    }

    if some_files_failed {
        eprintln!("Error: some files failed to extract; see above errors.");
        1
    } else {
        0
    }
}
