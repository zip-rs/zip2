//! Get the zip file infos
//!
//! ```sh
//! cargo run --example file_info my_file.zip
//! ```
//!

use std::fs;
use std::io::BufReader;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 2 {
        return Err(format!("Usage: {:?} <filename>", args[0]).into());
    }
    let fname_arg = &args[1];
    // Determine a trusted base directory (current working directory).
    let base_dir = std::env::current_dir()
        .map_err(|e| format!("Could not determine current directory: {e}"))?;
    // Construct the path relative to the trusted base directory and canonicalize it.
    let candidate_path = base_dir.join(fname_arg);
    // FIXME: still vulnerable to a Time-of-check to time-of-use (TOCTOU) race condition.
    //
    // An attacker could modify a path component (e.g., by replacing a directory with a symlink
    // between the canonicalize() and open() calls. This could bypass the starts_with check and lead
    // to path traversal. A fully secure solution is difficult without openat-like functionality
    // (which isn't in std).
    let path = candidate_path
        .canonicalize()
        .map_err(|e| format!("Could not open {fname_arg:?}: {e}"))?;
    if !path.starts_with(base_dir.canonicalize().unwrap_or(base_dir)) {
        return Err("Error: refusing to open path outside of base directory: {fname_arg:?}".into());
    }
    let mut archive = zip::ZipArchive::new(BufReader::new(fs::File::open(&path)?))
        .map_err(|e| format!("Could not open {fname_arg:?}: {e}"))?;
    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        let outpath = match file.enclosed_name() {
            Some(path) => path,
            None => {
                println!("Entry {:?} has a suspicious path", file.name());
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
                "Entry {} is a directory with name {:?}",
                i,
                outpath.display()
            );
        } else {
            println!(
                "Entry {} is a file with name {:?} ({} bytes)",
                i,
                outpath.display(),
                file.size()
            );
        }
    }

    Ok(())
}
