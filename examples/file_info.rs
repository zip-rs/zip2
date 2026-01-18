use anyhow::{anyhow, Context, Result};
use std::fs;
use std::io::BufReader;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 2 {
        return Err(anyhow!("Usage: {} <filename>", args[0]));
    }
    let fname_arg = &args[1];
    // Determine a trusted base directory (current working directory).
    let base_dir =
        std::env::current_dir().with_context(|| "Could not determine current directory")?;
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
        .with_context(|| format!("Could not open {fname_arg:?}"))?;
    if !path.starts_with(base_dir.canonicalize().unwrap_or(base_dir)) {
        return Err(anyhow!(
            "Error: refusing to open path outside of base directory: {fname_arg:?}"
        ));
    }
    let mut archive = zip::ZipArchive::new(BufReader::new(fs::File::open(&path)?))
        .with_context(|| format!("Could not open {fname_arg:?}"))?;
    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
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
                "Entry {} is a directory with name {:?}",
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
