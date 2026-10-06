// Create a ZIP with directory traversal attempts
// zipfile
//  - file_test
//  - dir/
//    - file_test_in_folder
//  - symlink -> file_test
//  - symlink_outside -> ../file_test
fn create_base_archive_to_extract() -> zip::ZipWriter<std::io::Cursor<Vec<u8>>> {
    use std::io::Write;
    use zip::CompressionMethod;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let buf = Vec::new();
    let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    writer.start_file("file_test", options).unwrap();
    writer.write_all(b"content").unwrap();
    writer.add_directory("dir/", options).unwrap();
    writer
        .start_file("dir/file_test_in_folder", options)
        .unwrap();
    writer.write_all(b"content").unwrap();

    writer.add_symlink("symlink", "file_test", options).unwrap();
    writer
        .add_symlink("symlink_outside", "../file_test", options)
        .unwrap();
    writer
}

// Create a ZIP with directory traversal attempts
// zipfile
//  - file_test
//  - dir/
//    - file_test_in_folder
//  - symlink -> file_test
//  - symlink_outside -> ../file_test
//  - symlink_root -> /
//  - symlink_root_tmp -> /tmp/not_here
fn create_archive_to_extract() -> Vec<u8> {
    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    let mut writer = create_base_archive_to_extract();
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.add_symlink("symlink_root", "/", options).unwrap();
    writer
        .add_symlink("symlink_root_tmp", "/tmp/not_here", options)
        .unwrap();
    writer.finish().unwrap().into_inner()
}

/// check if the symlink exists
fn is_a_symlink(path: &std::path::Path) -> bool {
    use std::fs::symlink_metadata;
    symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

#[cfg(not(miri))]
#[test]
fn test_extract_options_no_symlink() {
    use std::io::Cursor;
    use tempfile::TempDir;
    use zip::ZipArchive;
    use zip::read::ExtractOptions;
    use zip::read::SymlinkExtractAction;

    let buf = create_archive_to_extract();
    let reader = Cursor::new(buf);
    let mut archive = ZipArchive::new(reader).unwrap();
    let dest = TempDir::with_prefix("no_symlink").unwrap();

    archive
        .extract_with_options(
            &dest,
            ExtractOptions::default().symlink_action(SymlinkExtractAction::NoExtract),
        )
        .unwrap();
    assert!(dest.path().join("file_test").exists());
    assert!(dest.path().join("dir/").exists());
    assert!(dest.path().join("dir/file_test_in_folder").exists());
    assert!(!is_a_symlink(&dest.path().join("symlink"))); // not a symlink
    assert!(!dest.path().join("symlink").exists()); // NOT HERE
    assert!(!is_a_symlink(&dest.path().join("symlink_outside"))); // not a symlink
    assert!(!dest.path().join("symlink_outside").exists()); // NOT HERE
    assert!(!is_a_symlink(&dest.path().join("symlink_root"))); // not a symlink
    assert!(!dest.path().join("symlink_root").exists()); // NOT HERE
    assert!(!is_a_symlink(&dest.path().join("symlink_root_tmp"))); // not a symlink
    assert!(!dest.path().join("symlink_root_tmp").exists()); // NOT HERE
}

#[cfg(not(miri))]
#[test]
fn test_extract_options_symlink_file() {
    use std::io::Cursor;
    use tempfile::TempDir;
    use zip::ZipArchive;
    use zip::read::ExtractOptions;
    use zip::read::SymlinkExtractAction;

    let buf = create_archive_to_extract();
    let reader = Cursor::new(buf);
    let mut archive = ZipArchive::new(reader).unwrap();
    let dest = TempDir::with_prefix("no_symlink").unwrap();

    archive
        .extract_with_options(
            &dest,
            ExtractOptions::default().symlink_action(SymlinkExtractAction::ExtractAsFileUtf8),
        )
        .unwrap();
    assert!(dest.path().join("file_test").exists());
    assert!(dest.path().join("dir/").exists());
    assert!(dest.path().join("dir/file_test_in_folder").exists());
    assert!(dest.path().join("symlink").exists()); // present
    assert!(!dest.path().join("symlink").is_symlink()); // not a symlink
    assert!(dest.path().join("symlink_outside").exists()); // present
    assert!(!dest.path().join("symlink_outside").is_symlink()); // not a symlink
    assert!(dest.path().join("symlink_root").exists()); // present
    assert!(!dest.path().join("symlink_root").is_symlink()); // not a symlink
    assert!(dest.path().join("symlink_root_tmp").exists()); // present
    assert!(!dest.path().join("symlink_root_tmp").is_symlink()); // not a symlink
}

#[cfg(all(any(unix, windows), not(miri)))]
#[test]
fn test_extract_options_extract_in_folder() {
    use std::io::Cursor;
    use tempfile::TempDir;
    use zip::ZipArchive;
    use zip::read::ExtractOptions;
    use zip::read::SymlinkExtractAction;

    let writer = create_base_archive_to_extract();
    let buf = writer.finish().unwrap().into_inner();
    let reader = Cursor::new(buf);
    let mut archive = ZipArchive::new(reader).unwrap();
    let dest = TempDir::with_prefix("no_symlink").unwrap();

    let res = archive.extract_with_options(
        &dest,
        ExtractOptions::default().symlink_action(SymlinkExtractAction::ExtractInFolder),
    );
    assert!(res.is_err());
    // error because symlink_outside target is outside
    assert!(!dest.path().join("symlink_outside").exists()); // NOT HERE
}

#[cfg(all(any(unix, windows), not(miri)))]
#[test]
fn test_extract_options_no_restrictions() {
    use std::fs::read_link;
    use std::io::Cursor;
    use std::path::PathBuf;

    use tempfile::TempDir;
    use zip::ZipArchive;
    use zip::read::ExtractOptions;
    use zip::read::SymlinkExtractAction;

    let buf = create_archive_to_extract();
    let reader = Cursor::new(buf);
    let mut archive = ZipArchive::new(reader).unwrap();
    let dest = TempDir::with_prefix("no_symlink").unwrap();

    archive
        .extract_with_options(
            &dest,
            ExtractOptions::default().symlink_action(SymlinkExtractAction::ExtractNoRestrictions),
        )
        .unwrap();
    assert!(dest.path().join("file_test").exists());
    assert!(dest.path().join("dir/").exists());
    assert!(dest.path().join("dir/file_test_in_folder").exists());

    assert!(is_a_symlink(&dest.path().join("symlink"))); // is a symlink
    assert!(dest.path().join("symlink").is_symlink()); // is a symlink
    assert!(dest.path().join("symlink").exists()); // target exists

    assert!(is_a_symlink(&dest.path().join("symlink_outside")));
    assert!(dest.path().join("symlink_outside").is_symlink());
    assert!(!dest.path().join("symlink_outside").exists()); // target does not exist
    assert_eq!(
        read_link(dest.path().join("symlink_outside")).unwrap(),
        PathBuf::from("../file_test")
    );

    assert!(is_a_symlink(&dest.path().join("symlink_root")));
    assert!(dest.path().join("symlink_root").is_symlink());
    assert!(dest.path().join("symlink_root").exists()); // target (/) exists
    assert_eq!(
        read_link(dest.path().join("symlink_root")).unwrap(),
        PathBuf::from("/")
    );

    assert!(is_a_symlink(&dest.path().join("symlink_root_tmp")));
    assert!(dest.path().join("symlink_root_tmp").is_symlink());
    assert!(!dest.path().join("symlink_root_tmp").exists()); // target does not exist
    assert_eq!(
        read_link(dest.path().join("symlink_root_tmp")).unwrap(),
        PathBuf::from("/tmp/not_here")
    );
}

#[cfg(all(any(unix, windows), not(miri)))]
#[test]
fn test_extract_options_recursive_in_folder() {
    use std::fs::read_link;
    use std::io::Cursor;
    use std::path::PathBuf;

    use tempfile::TempDir;
    use zip::ZipArchive;
    use zip::read::ExtractOptions;
    use zip::read::SymlinkExtractAction;

    let writer = create_base_archive_to_extract();
    let buf = writer.finish().unwrap().into_inner();
    let reader = Cursor::new(buf);
    let mut archive = ZipArchive::new(reader).unwrap();
    let dest = TempDir::with_prefix("no_symlink").unwrap();

    let final_dest = dest.path().join("extracted");
    std::fs::create_dir_all(&final_dest).unwrap();
    #[cfg(unix)]
    {
        // create a symlink to the inside
        std::os::unix::fs::symlink("extracted", dest.path().join("extracted_link")).unwrap();
    }
    #[cfg(windows)]
    {
        // create a symlink to the inside
        std::os::windows::fs::symlink_file("extracted", final_dest.join("extracted_link")).unwrap();
    }

    archive
        .extract_with_options(
            &final_dest,
            ExtractOptions::default()
                .symlink_action(SymlinkExtractAction::ExtractRecursiveInFolder),
        )
        .unwrap();

    assert!(final_dest.join("file_test").exists());
    assert!(final_dest.join("dir/").exists());
    assert!(final_dest.join("dir/file_test_in_folder").exists());

    assert!(is_a_symlink(&final_dest.join("symlink"))); // is a symlink
    assert!(final_dest.join("symlink").is_symlink()); // is a symlink
    assert!(final_dest.join("symlink").exists()); // target exists

    assert!(is_a_symlink(&final_dest.join("symlink_outside")));
    assert!(final_dest.join("symlink_outside").is_symlink());
    assert!(final_dest.join("symlink_outside").exists()); // target does exist
    assert_eq!(
        read_link(final_dest.join("symlink_outside")).unwrap(),
        PathBuf::from("../file_test")
    );
}

#[cfg(all(any(unix, windows), not(miri)))]
#[test]
fn test_extract_options_recursive_in_folder_non_working() {
    use std::io::Cursor;

    use tempfile::TempDir;
    use zip::ZipArchive;
    use zip::read::ExtractOptions;
    use zip::read::SymlinkExtractAction;

    let writer = create_base_archive_to_extract();
    let buf = writer.finish().unwrap().into_inner();
    let reader = Cursor::new(buf);
    let mut archive = ZipArchive::new(reader).unwrap();
    let dest = TempDir::with_prefix("no_symlink").unwrap();

    let final_dest = dest.path().join("extracted");
    std::fs::create_dir_all(&final_dest).unwrap();
    #[cfg(unix)]
    {
        // create a symlink to the outside
        std::os::unix::fs::symlink("/", dest.path().join("file_test")).unwrap();
    }
    #[cfg(windows)]
    {
        // create a symlink to the outside
        std::os::windows::fs::symlink_file("/", final_dest.join("file_test")).unwrap();
    }

    let res = archive.extract_with_options(
        &dest,
        ExtractOptions::default().symlink_action(SymlinkExtractAction::ExtractRecursiveInFolder),
    );
    assert!(res.is_err());
}
