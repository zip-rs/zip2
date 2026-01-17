use std::error::Error;
use std::fs;
use std::io::BufReader;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <filename>", args[0]);
        return Err("Wrong usage".into());
    }
    let fname_arg = &args[1];
    let fname_path = std::path::Path::new(fname_arg);
    // Basic validation to guard against unsafe paths.
    // Reject absolute paths and any path containing parent directory references.
    if fname_path.is_absolute() || fname_arg.contains("..") {
        eprintln!("Error: refusing to open unsafe path \"{}\"", fname_arg);
        return Err("Unsafe path".into());
    }
    let fname = fname_path;
    let file = fs::File::open(fname).unwrap();
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

    Ok(())
}
