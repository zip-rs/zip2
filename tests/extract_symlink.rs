#[test]
#[cfg(all(unix, feature = "_deflate-any"))]
fn extract_should_respect_links() {
    use std::{fs, io, path::PathBuf, str::FromStr};
    use tempdir::TempDir;
    use zip::ZipArchive;

    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/pandoc_soft_links.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");
    let temp_dir = TempDir::new("pandoc_soft_links").unwrap();
    archive.extract(&temp_dir).unwrap();

    let symlink_path = temp_dir.path().join("pandoc-3.2-arm64/bin/pandoc-lua");

    // Read the target of the symbolic link
    let target_path = fs::read_link(&symlink_path).unwrap();

    assert_eq!(target_path, PathBuf::from_str("pandoc").unwrap());
}
