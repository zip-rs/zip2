use std::error::Error;
use std::io::{ErrorKind, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
#[cfg(feature = "aes-crypto")]
use zip::{AesMode, CompressionMethod};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <filename>", args[0]);
        return Err("Wrong usage".into());
    }

    let filename = &args[1];
    match write_zip_file(filename) {
        Ok(_) => {
            println!("File written to {filename}");
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: {e:?}");
            Err(e.into())
        }
    }
}

fn write_zip_file(filename: &str) -> zip::result::ZipResult<()> {
    let path = Path::new(filename);

    // Validate that the provided filename does not escape the current directory
    if path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        // Return an error instead of writing to an arbitrary location
        return Err(invalid!(
            "unsafe output path: attempted directory traversal or absolute path",
        ));
    }

    // Create the file relative to the current working directory
    let base = std::env::current_dir().map_err(|_| {
        zip::result::ZipError::Io(std::io::Error::new(
            ErrorKind::NotFound,
            "Failed to get current directory",
        ))
    })?;
    let safe_path = base.join(path);

    let file = std::fs::File::create(safe_path)?;

    let mut zip = zip::ZipWriter::new(file);

    zip.add_directory("test/", SimpleFileOptions::default())?;

    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o755);
    zip.start_file("test/☃.txt", options)?;
    zip.write_all(b"Hello, World!\n")?;

    zip.start_file("test/lorem_ipsum.txt", options)?;
    zip.write_all(LOREM_IPSUM)?;

    #[cfg(feature = "aes-crypto")]
    {
        zip.start_file(
            "test/lorem_ipsum.aes.txt",
            options
                .compression_method(CompressionMethod::Zstd)
                .with_aes_encryption(AesMode::Aes256, "password"),
        )?;
        zip.write_all(LOREM_IPSUM)?;

        // This should use AE-1 due to the short file length.
        zip.start_file(
            "test/short.aes.txt",
            options.with_aes_encryption(AesMode::Aes256, "password"),
        )?;
        zip.write_all(b"short text\n")?;
    }

    zip.finish()?;
    Ok(())
}

const LOREM_IPSUM: &[u8] = b"Lorem ipsum dolor sit amet, consectetur adipiscing elit.\n\
Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.\n\
Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.\n\
Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur.\n\
Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.\n";
