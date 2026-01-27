//! Example to write a zip dir
//!
//! ```sh
//! cargo run --example write_dir src/ dest.zip xz
//! ```

use clap::{Parser, ValueEnum};
use walkdir::WalkDir;
use zip::{cfg_if_expr, result::ZipError, write::SimpleFileOptions};

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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let src_dir = &args.source;
    let dest_file = &args.destination;
    let method: Result<zip::CompressionMethod, Box<dyn std::error::Error>> =
        match args.compression_method {
            CompressionMethod::Stored => Ok(zip::CompressionMethod::Stored),
            CompressionMethod::Deflated => cfg_if_expr! {
                #[cfg(feature = "_deflate-any")] => Ok(zip::CompressionMethod::Deflated),
                _ => Err("The `deflate-*` features are not enabled".into()),
            },
            CompressionMethod::Bzip2 => cfg_if_expr! {
                #[cfg(feature = "_bzip2_any")] => Ok(zip::CompressionMethod::Bzip2),
                _ => Err("The `bzip2-*` features are not enabled".into()),
            },
            CompressionMethod::Xz => cfg_if_expr! {
                #[cfg(feature = "xz")] => Ok(zip::CompressionMethod::Xz),
                _ => Err("The `xz` feature is not enabled".into()),
            },
            CompressionMethod::Zstd => cfg_if_expr! {
                #[cfg(feature = "zstd")] => Ok(zip::CompressionMethod::Zstd),
                _ => Err("The `zstd` feature is not enabled".into()),
            },
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
