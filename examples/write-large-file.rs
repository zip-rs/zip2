//! Write a huge file with lots of zeros, that should compress perfectly.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if !cfg!(feature = "_deflate-any") {
        return Err("Please enable one of the deflate features".into());
    }
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 2 {
        return Err(format!("Usage: {} <filename>", args[0]).into());
    }

    #[cfg(feature = "_deflate-any")]
    {
        let filename = &*args[1];
        // Ensure that the filename is non-empty and has no path separators or parent directory references
        let trimmed = filename.trim();
        if trimmed.is_empty()
            || trimmed.contains("..")
            || trimmed.contains('/')
            || trimmed.contains('\\')
        {
            return Err("Invalid filename: must be a non-empty simple file name without path separators or '..'".into());
        }
        let filename = trimmed;
        use std::io::Write;

        use zip::write::SimpleFileOptions;

        let file = std::fs::File::create(filename)?;
        let mut zip = zip::ZipWriter::new(file);

        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            // files over u32::MAX require this flag set.
            .large_file(true)
            .unix_permissions(0o755);
        zip.start_file("huge-file-of-zeroes", options)?;
        let content: Vec<_> = std::iter::repeat_n(0_u8, 65 * 1024).collect();
        let mut bytes_written = 0_u64;
        while bytes_written < u32::MAX as u64 {
            zip.write_all(&content)?;
            bytes_written += content.len() as u64;
        }
        zip.finish()?;
    }
    Ok(())
}
