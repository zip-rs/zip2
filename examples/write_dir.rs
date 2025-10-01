#![allow(unused_variables)]
#![allow(dead_code)]
use anyhow::Context;
use clap::{Parser, ValueEnum};
use std::io::prelude::*;
use zip::{cfg_if_expr, result::ZipError, write::SimpleFileOptions};

use std::fs::File;
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

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

fn main() -> ! {
    let args = Args::parse();
    let src_dir = &args.source;
    let dst_file = &args.destination;
    let method = match args.compression_method {
        CompressionMethod::Stored => zip::CompressionMethod::Stored,
        CompressionMethod::Deflated => cfg_if_expr! {
            #[cfg(feature = "deflate-flate2")] => zip::CompressionMethod::Deflated,
            _ => {
                println!("The `deflate-flate2` feature is not enabled");
                std::process::exit(1)
            }
        },
        CompressionMethod::Bzip2 => cfg_if_expr! {
            #[cfg(feature = "bzip2")] => zip::CompressionMethod::Bzip2,
            _ => {
                println!("The `bzip2` feature is not enabled");
                std::process::exit(1)
            }
        },
        CompressionMethod::Xz => cfg_if_expr! {
            #[cfg(feature = "xz")] => zip::CompressionMethod::Xz,
            _ => {
                println!("The `xz` feature is not enabled");
                std::process::exit(1)
            }
        },
        CompressionMethod::Zstd => cfg_if_expr! {
            #[cfg(feature = "zstd")] => zip::CompressionMethod::Zstd,
            _ => {
                println!("The `zstd` feature is not enabled");
                std::process::exit(1)
            }
        },
    };
    match doit(src_dir, dst_file, method) {
        Ok(_) => {
            println!("done: {src_dir:?} written to {dst_file:?}");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("Error: {e:?}");
            std::process::abort();
        }
    }
}

fn zip_dir<T>(
    it: &mut dyn Iterator<Item = DirEntry>,
    prefix: &Path,
    writer: T,
    method: zip::CompressionMethod,
) -> anyhow::Result<()>
where
    T: Write + Seek,
{
    let mut zip = zip::ZipWriter::new(writer);
    let options = SimpleFileOptions::default()
        .compression_method(method)
        .unix_permissions(0o755);

    let prefix = Path::new(prefix);
    let mut buffer = Vec::new();
    for entry in it {
        let path = entry.path();
        let name = path.strip_prefix(prefix).unwrap();
        let path_as_string = name
            .to_str()
            .map(str::to_owned)
            .with_context(|| format!("{name:?} Is a Non UTF-8 Path"))?;

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

fn doit(src_dir: &Path, dst_file: &Path, method: zip::CompressionMethod) -> anyhow::Result<()> {
    if !Path::new(src_dir).is_dir() {
        return Err(ZipError::FileNotFound.into());
    }

    let path = Path::new(dst_file);
    let file = File::create(path).unwrap();

    let walkdir = WalkDir::new(src_dir);
    let it = walkdir.into_iter();

    zip_dir(&mut it.filter_map(|e| e.ok()), src_dir, file, method)?;

    Ok(())
}
