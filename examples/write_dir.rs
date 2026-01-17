use anyhow::{anyhow, Result};
use clap::{Parser, ValueEnum};
use walkdir::WalkDir;
use zip::{cfg_if_expr, result::ZipError, write::SimpleFileOptions};

use std::fs::File;
use std::io::{Read, Write};
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

fn main() -> Result<()> {
    let args = Args::parse();
    let src_dir = &args.source;
    let dst_file = &args.destination;
    let method = match args.compression_method {
        CompressionMethod::Stored => zip::CompressionMethod::Stored,
        CompressionMethod::Deflated => cfg_if_expr! {
            #[cfg(feature = "_deflate-any")] => zip::CompressionMethod::Deflated,
            _ => return Err(anyhow!("The `deflate-flate2` features are not enabled")),
        },
        CompressionMethod::Bzip2 => cfg_if_expr! {
            #[cfg(feature = "_bzip2_any")] => zip::CompressionMethod::Bzip2,
            _ => return Err(anyhow!("The `bzip2` features are not enabled")),
        },
        CompressionMethod::Xz => cfg_if_expr! {
            #[cfg(feature = "xz")] => zip::CompressionMethod::Xz,
            _ => return Err(anyhow!("The `xz` feature is not enabled")),
        },
        CompressionMethod::Zstd => cfg_if_expr! {
            #[cfg(feature = "zstd")] => zip::CompressionMethod::Zstd,
            _ => return Err(anyhow!("The `zstd` feature is not enabled")),
        },
    };
    zip_dir(src_dir, dst_file, method)?;
    println!("done: {src_dir:?} written to {dst_file:?}");
    Ok(())
}

fn zip_dir(src_dir: &Path, dst_file: &Path, method: zip::CompressionMethod) -> Result<()> {
    if !Path::new(src_dir).is_dir() {
        return Err(ZipError::FileNotFound.into());
    }

    let path = Path::new(dst_file);
    let file = File::create(path)?;

    let walkdir = WalkDir::new(src_dir);

    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(method)
        .unix_permissions(0o755);

    let prefix = Path::new(src_dir);
    let mut buffer = Vec::new();
    for entry in walkdir.into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        let name = path.strip_prefix(prefix)?;
        let path_as_string = name
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("{name:?} is a Non UTF-8 Path"))?;

        // Write file or directory explicitly
        // Some unzip tools unzip files with directory paths correctly, some do not!
        if path.is_file() {
            println!("adding file {path:?} as {name:?} ...");
            zip.start_file(path_as_string, options)?;
            let mut f = File::open(path)?;

            f.read_to_end(&mut buffer)?;
            zip.write_all(&buffer)?;
            buffer.clear();
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
