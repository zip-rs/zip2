// See this dicussion for further background on why it is done like this:
// https://github.com/zip-rs/zip/discussions/430

use std::io::Write;
use anyhow::{anyhow, Result};
use zip::result::{ZipError, ZipResult};
use zip::write::SimpleFileOptions;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 3 {
        return Err(anyhow!(
            "Usage: {:?} <filename> <file_within_archive_to_update>",
            args[0]
        ));
    }
    let filename = &*args[1];
    let file_to_update = &*args[2];
    update_file(filename, file_to_update, false)?;
    Ok(())
}

fn update_file(archive_filename: &str, file_to_update: &str, in_place: bool) -> ZipResult<()> {
    // Validate the archive path:
    //  - Disallow absolute paths.
    //  - Disallow parent directory components.
    //  - Ensure the resolved path stays within the current working directory.
    let raw_path = std::path::Path::new(archive_filename);
    if raw_path.is_absolute()
        || raw_path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(ZipError::FileNotFound);
    }

    // Use the current working directory as the base for archives.
    let base_dir = std::env::current_dir()?;
    let joined = base_dir.join(raw_path);
    let archive_path = joined.canonicalize()?;
    if !archive_path.starts_with(&base_dir) {
        return Err(ZipError::FileNotFound);
    }

    let zipfile = std::fs::File::open(&archive_path)?;

    let mut archive = zip::ZipArchive::new(zipfile)?;

    // Open a new, empty archive for writing to
    let new_filename = replacement_filename(&archive_path)?;
    let new_file = std::fs::File::create(&new_filename)?;
    let mut new_archive = zip::ZipWriter::new(new_file);

    // Loop through the original archive:
    //  - Write the target file from some bytes
    //  - Copy everything else across as raw, which saves the bother of decoding it
    // The end effect is to have a new archive, which is a clone of the original,
    // save for the target file which has been re-written
    let target: &std::path::Path = file_to_update.as_ref();
    let new = b"Lorem ipsum";
    for i in 0..archive.len() {
        let file = archive.by_index_raw(i)?;
        match file.enclosed_name() {
            Some(p) if p == target => {
                new_archive.start_file(file_to_update, SimpleFileOptions::default())?;
                new_archive.write_all(new)?;
                new_archive.flush()?;
            }
            _ => new_archive.raw_copy_file(file)?,
        }
    }
    new_archive.finish()?;

    drop(archive);

    // If we're doing this in place then overwrite the original with the new
    if in_place {
        std::fs::rename(&new_filename, &archive_path)?;
    }

    Ok(())
}

fn replacement_filename(source: &std::path::Path) -> ZipResult<std::path::PathBuf> {
    let mut new = std::path::PathBuf::from(source);
    let mut stem = source.file_stem().ok_or(ZipError::FileNotFound)?.to_owned();
    stem.push("_updated");
    new.set_file_name(stem);
    let ext = source.extension().ok_or(ZipError::FileNotFound)?;
    new.set_extension(ext);
    Ok(new)
}
