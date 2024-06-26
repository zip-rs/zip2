use std::fs;

use walkdir::WalkDir;

#[cfg(all(unix, feature = "_deflate-any"))]
#[test]
fn repro_old423() -> zip::result::ZipResult<()> {
    use std::io;
    use tempdir::TempDir;
    use zip::ZipArchive;

    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/lin-ub_iwd-v11.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");
    archive.extract(TempDir::new("repro_old423")?)
}

#[test]
fn extract_should_respect_links(){
    use std::io;
    use tempdir::TempDir;
    use zip::ZipArchive;

    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/pandoc_soft_links.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");
    let temp_dir = TempDir::new("pandoc_soft_links").unwrap();
    archive.extract(&temp_dir).unwrap();

    
    let symlink_path = temp_dir.path().join("pandoc-3.2-arm64/bin/pandoc-lua");
    
    // Check if the file is a symbolic link
    let metadata = fs::symlink_metadata(&symlink_path).unwrap();

    // assert!(metadata.is_symlink());

    // Read the target of the symbolic link
    let target_path = fs::read_link(&symlink_path).unwrap();
    eprintln!("Symbolic link points to: {:?}", target_path);



}
