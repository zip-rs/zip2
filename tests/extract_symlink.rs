/// Only on little endian because we cannot use fs with miri CI
#[cfg(all(target_endian = "little", not(miri)))]
#[test]
fn test_is_symlink() -> std::io::Result<()> {
    use std::io::Cursor;
    use tempfile::TempDir;
    use zip::ZipArchive;

    let mut reader = ZipArchive::new(Cursor::new(include_bytes!("../tests/data/symlink.zip")))?;
    assert!(reader.by_index(0)?.is_symlink());
    let tempdir = TempDir::with_prefix("test_is_symlink")?;
    reader.extract(&tempdir)?;
    assert!(tempdir.path().join("bar").is_symlink());
    Ok(())
}

#[test]
#[cfg(all(unix, feature = "deflate-flate2"))]
fn extract_should_respect_links() {
    use std::{fs, io, path::PathBuf, str::FromStr};
    use tempfile::TempDir;
    use zip::ZipArchive;

    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/pandoc_soft_links.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");
    let temp_dir = TempDir::with_prefix("pandoc_soft_links").unwrap();
    archive.extract(&temp_dir).unwrap();

    let symlink_path = temp_dir.path().join("pandoc-3.2-arm64/bin/pandoc-lua");

    // Read the target of the symbolic link
    let target_path = fs::read_link(&symlink_path).unwrap();

    assert_eq!(target_path, PathBuf::from_str("pandoc").unwrap());
}

/// Symlinks being extracted shouldn't be followed out of the destination directory.
/// Only on little endian because we cannot use fs with miri CI
#[cfg(all(target_endian = "little", not(miri)))]
#[test]
fn test_cannot_symlink_outside_destination() -> zip::result::ZipResult<()> {
    use std::fs::create_dir;
    use std::io::Cursor;
    use tempfile::TempDir;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.add_symlink("symlink/", "../dest-sibling/", SimpleFileOptions::default())?;
    writer.start_file("symlink/dest-file", SimpleFileOptions::default())?;
    let mut reader = writer.finish_into_readable()?;
    let dest_parent = TempDir::with_prefix("read__test_cannot_symlink_outside_destination")?;
    let dest_sibling = dest_parent.path().join("dest-sibling");
    create_dir(&dest_sibling)?;
    let dest = dest_parent.path().join("dest");
    create_dir(&dest)?;
    assert!(reader.extract(dest).is_err());
    assert!(!dest_sibling.join("dest-file").exists());
    Ok(())
}
