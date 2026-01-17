use std::error::Error;
use std::io::{self, Write};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    str::FromStr,
};
use zip::write::SimpleFileOptions;

fn gather_files<'a, T: Into<&'a Path>>(path: T, base: &Path, files: &mut Vec<PathBuf>) {
    let path: &Path = path.into();

    for entry in path.read_dir().unwrap() {
        match entry {
            Ok(e) => {
                let entry_path = e.path();
                let canonical = match entry_path.canonicalize() {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                if !canonical.starts_with(base) {
                    // Skip entries that resolve outside the allowed base directory.
                    continue;
                }

                if canonical.is_dir() {
                    gather_files(canonical.as_path(), base, files);
                } else if canonical.is_file() {
                    files.push(canonical);
                }
            }
            Err(e) => {
                eprintln!("Warning: Failed to read directory entry: {}", e);
                continue;
            }
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <existing archive> <folder_to_append>", args[0]);
        return Err("Wrong usage".into());
    }

    let existing_archive_path = &*args[1];
    let append_dir_path = &*args[2];
    let archive = PathBuf::from_str(existing_archive_path).unwrap();
    let base_dir = match std::env::current_dir() {
        Ok(dir) => match dir.canonicalize() {
            Ok(c) => c,
            Err(e) => {
                let _ = writeln!(io::stderr(), "Failed to canonicalize base directory: {}", e);
                return Err(e.into());
            }
        },
        Err(e) => {
            let _ = writeln!(io::stderr(), "Failed to determine current directory: {}", e);
            return Err(e.into());
        }
    };

    let to_append = match PathBuf::from_str(append_dir_path) {
        Ok(path) => {
            if path.is_absolute() {
                return Err("Absolute paths are not allowed".into());
            }
            if path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err("Parent directory references (..) are not allowed".into());
            }
            base_dir.join(path)
        }
        Err(e) => {
            let _ = writeln!(io::stderr(), "Invalid path: {}", e);
            return Err(e.into());
        }
    };
    let to_append = match to_append.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(
                io::stderr(),
                "Failed to canonicalize append directory: {}",
                e
            );
            return Err(e.into());
        }
    };

    if !to_append.starts_with(&base_dir) {
        return Err("Refusing to append from directory outside allowed base".into());
    }

    let existing_zip = OpenOptions::new()
        .read(true)
        .write(true)
        .open(archive)
        .unwrap();
    let mut append_zip = zip::ZipWriter::new_append(existing_zip).unwrap();

    let mut files: Vec<PathBuf> = vec![];
    gather_files(to_append.as_ref(), &base_dir, &mut files);

    for file in files {
        append_zip
            .start_file(file.to_string_lossy(), SimpleFileOptions::default())
            .unwrap();

        let mut f = File::open(file).unwrap();
        let _ = std::io::copy(&mut f, &mut append_zip);
    }

    append_zip.finish().unwrap();

    Ok(())
}
