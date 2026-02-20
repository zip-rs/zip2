//! Example to write a zip dir
//!
//! ```sh
//! cargo run --example write_dir src/ dest.zip xz
//! ```

use clap::{Parser, ValueEnum};
use walkdir::WalkDir;
use zip::{result::ZipError, write::SimpleFileOptions};

use std::fs::File;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(about, long_about = None)]
struct Args {
    // Source directory
    source: PathBuf,
    // Destination zipfile
    destination: PathBuf,
    // Compression method
    #[arg(value_enum)]
    compression_method: CompressionMethod,
}

#[derive(Clone, ValueEnum)]
enum CompressionMethod {
    Stored,
    Deflated,
    Bzip2,
    Xz,
    Zstd,
}

/// Used to test with --no-default-features or a specific features
/// ```sh
/// cargo run --no-default-features --example write_dir src/ dest.zip xz
/// # should error because no xz
///
/// cargo run --features xz --example write_dir src/ dest.zip xz
/// # should work
/// ```
macro_rules! is_feature {
    ($feature:literal, $compression:expr) => {{
        #[cfg(feature = $feature)]
        {
            Ok($compression)
        }

        #[cfg(not(feature = $feature))]
        {
            Err(format!("The `{}` feature is not enabled", $feature).into())
        }
    }};
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let src_dir = &args.source;
    let dest_file = &args.destination;
    let method: Result<zip::CompressionMethod, Box<dyn std::error::Error>> =
        match args.compression_method {
            CompressionMethod::Stored => Ok(zip::CompressionMethod::Stored),
            // _deflate-any and _bzip2_any are enabled by their respective implementations
            CompressionMethod::Deflated => {
                is_feature!("_deflate-any", zip::CompressionMethod::Deflated)
            }
            CompressionMethod::Bzip2 => is_feature!("dep:bzip2", zip::CompressionMethod::Bzip2),
            CompressionMethod::Xz => is_feature!("xz", zip::CompressionMethod::Xz),
            CompressionMethod::Zstd => is_feature!("zstd", zip::CompressionMethod::Zstd),
        };
    let method = method?;
    zip_dir(src_dir, dest_file, method)?;
    println!("done: {src_dir:?} written to {dest_file:?}");
    Ok(())
}

fn zip_dir(
    src_dir: &Path,
    dest_file: &Path,
    method: zip::CompressionMethod,
) -> Result<(), Box<dyn std::error::Error>> {
    if !Path::new(src_dir).is_dir() {
        return Err(ZipError::FileNotFound.into());
    }

    if dest_file.exists() {
        return Err(format!("File {} already exists", dest_file.display()).into());
    }
    let file = File::create(dest_file)?;

    let walkdir = WalkDir::new(src_dir);

    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(method)
        .unix_permissions(0o755);

    // SECURITY NOTE: Any error after this point may leave a partial or corrupt
    // zip file.
    // This can lead to data integrity issues or race conditions (e.g., TOCTOU)
    // if other processes access the incomplete file. A robust application
    // should mitigate this, for example by writing to a temporary file and
    // renaming it on success to ensure atomicity.
    for entry_result in walkdir.into_iter() {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(e) => {
                return Err(format!("Error while traversing directory {src_dir:?}: {e}").into());
            }
        };
        let path = entry.path();
        let name = path.strip_prefix(src_dir)?;
        let path_as_string = name
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("{name:?} is a Non UTF-8 Path"))?;

        // Write file or directory explicitly
        // Some unzip tools unzip files with directory paths correctly, some do not!
        if path.is_file() {
            println!("adding file {path:?} as {name:?} ...");
            zip.start_file(path_as_string, options)?;
            let mut f = File::open(path)?;

            std::io::copy(&mut f, &mut zip)?;
        } else if !name.as_os_str().is_empty() {
            // Only if not root! Avoids path spec / warning
            // and mapname conversion failed error on unzip
            println!("adding dir {path_as_string:?} as {name:?} ...");
            zip.add_directory(path_as_string, options)?;
        }
    }
    zip.finish()?;
    Ok(())
}
