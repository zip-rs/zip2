#[cfg(all(
    test,
    not(all(feature = "deflate-zopfli", not(feature = "deflate-flate2")))
))]
pub mod tests {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::{ZipArchive, ZipWriter};

    /// Create a ZIP file with entries that have absolute paths (starting with /)
    /// This simulates the problematic ZIP file mentioned in the bug report
    fn create_zip_with_absolute_paths() -> Vec<u8> {
        let buf = Vec::new();
        let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
        let options = SimpleFileOptions::default();

        // Create entries with absolute paths to simulate problematic ZIPs; creation is expected to succeed
        writer
            .add_directory("/_/", options)
            .expect("creating directory with absolute path should succeed");
        writer
            .start_file("/_/file1.txt", options)
            .expect("starting file with absolute path should succeed");
        writer.write_all(b"File 1 content").unwrap();
        writer
            .start_file("/_/subdir/file2.txt", options)
            .expect("starting nested file with absolute path should succeed");
        writer.write_all(b"File 2 content").unwrap();

        writer.finish().unwrap().into_inner()
    }

    /// Create a ZIP file with entries that have Windows-style absolute paths
    fn create_zip_with_windows_absolute_paths() -> Vec<u8> {
        let buf = Vec::new();
        let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
        let options = SimpleFileOptions::default();

        // Create entries with Windows absolute paths; creation is expected to succeed
        writer
            .add_directory("C:\\temp\\", options)
            .expect("creating directory with Windows absolute path should succeed");
        writer
            .start_file("C:\\temp\\file1.txt", options)
            .expect("starting file with Windows absolute path should succeed");
        writer.write_all(b"File 1 content").unwrap();

        writer.finish().unwrap().into_inner()
    }

    /// Create a ZIP file that more closely simulates the soldeer registry issue
    /// with an underscore directory at the root with absolute path
    fn create_zip_like_soldeer_issue() -> Vec<u8> {
        let buf = Vec::new();
        let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
        let options = SimpleFileOptions::default();

        // Simulate the soldeer registry structure with absolute paths; creation is expected to succeed
        writer
            .add_directory("/_/", options)
            .expect("creating root '_' directory with absolute path should succeed");
        writer
            .add_directory("/_/forge-std/", options)
            .expect("creating 'forge-std' directory with absolute path should succeed");
        writer
            .start_file("/_/forge-std/src/Test.sol", options)
            .expect("starting Test.sol file with absolute path should succeed");
        writer
            .write_all(b"// SPDX-License-Identifier: MIT\npragma solidity ^0.8.0;\n")
            .unwrap();
        writer
            .start_file("/_/forge-std/lib/ds-test/src/test.sol", options)
            .expect("starting nested ds-test file with absolute path should succeed");
        writer.write_all(b"// Test contract\n").unwrap();

        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn test_extract_zip_with_absolute_paths() {
        let zip_data = create_zip_with_absolute_paths();
        let mut archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();

        // After fix: should extract successfully, stripping the leading /
        let temp_dir = tempfile::TempDir::new().unwrap();
        archive.extract(temp_dir.path()).unwrap();

        // Files should be extracted with the absolute path prefix stripped
        assert!(temp_dir.path().join("_").exists());
        assert!(temp_dir.path().join("_/file1.txt").exists());
        assert!(temp_dir.path().join("_/subdir/file2.txt").exists());

        // Verify file contents
        let content = std::fs::read_to_string(temp_dir.path().join("_/file1.txt")).unwrap();
        assert_eq!(content, "File 1 content");
    }

    #[test]
    fn test_extract_zip_with_windows_absolute_paths() {
        let zip_data = create_zip_with_windows_absolute_paths();
        let mut archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();

        // After fix: should extract successfully, stripping the C:\ prefix
        let temp_dir = tempfile::TempDir::new().unwrap();
        archive.extract(temp_dir.path()).unwrap();

        // Files should be extracted with the Windows absolute path prefix stripped
        assert!(temp_dir.path().join("temp").exists());
        assert!(temp_dir.path().join("temp/file1.txt").exists());

        // Verify file contents
        let content = std::fs::read_to_string(temp_dir.path().join("temp/file1.txt")).unwrap();
        assert_eq!(content, "File 1 content");
    }

    #[test]
    fn test_extract_soldeer_like_zip() {
        let zip_data = create_zip_like_soldeer_issue();
        let mut archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();

        // This should now work without "Invalid file path" error
        let temp_dir = tempfile::TempDir::new().unwrap();
        archive.extract(temp_dir.path()).unwrap();

        // Verify the structure is extracted correctly with absolute prefix stripped
        assert!(temp_dir.path().join("_").exists());
        assert!(temp_dir.path().join("_/forge-std").exists());
        assert!(temp_dir.path().join("_/forge-std/src/Test.sol").exists());
        assert!(
            temp_dir
                .path()
                .join("_/forge-std/lib/ds-test/src/test.sol")
                .exists()
        );

        // Verify file contents
        let content =
            std::fs::read_to_string(temp_dir.path().join("_/forge-std/src/Test.sol")).unwrap();
        assert!(content.contains("SPDX-License-Identifier"));
    }

    #[test]
    fn test_individual_file_access_with_absolute_paths() {
        let zip_data = create_zip_with_absolute_paths();
        let mut archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();

        // Test accessing individual files
        for i in 0..archive.len() {
            let file = archive.by_index(i).unwrap();

            // After our fix, enclosed_name should return a safe relative path
            let enclosed_name = file.enclosed_name();

            // Should now return Some with the absolute prefix stripped
            assert!(enclosed_name.is_some());
            let path = enclosed_name.unwrap();

            // Verify the path doesn't start with / or contain absolute components
            assert!(!path.is_absolute());
            assert!(!path.to_string_lossy().starts_with('/'));
        }
    }

    #[test]
    fn test_security_still_prevents_directory_traversal() {
        let buf = Vec::new();
        let mut writer = ZipWriter::new(std::io::Cursor::new(buf));
        let options = SimpleFileOptions::default();

        // Create a ZIP with directory traversal attempts
        writer.start_file("../../../etc/passwd", options).unwrap();
        writer.write_all(b"malicious content").unwrap();
        writer
            .start_file("foo/../../../etc/shadow", options)
            .unwrap();
        writer.write_all(b"more malicious content").unwrap();

        let zip_data = writer.finish().unwrap().into_inner();
        let mut archive = ZipArchive::new(std::io::Cursor::new(zip_data)).unwrap();

        // These should still fail due to directory traversal protection
        for i in 0..archive.len() {
            let file = archive.by_index(i).unwrap();
            let enclosed_name = file.enclosed_name();

            // Directory traversal attempts should still return None
            assert!(
                enclosed_name.is_none(),
                "Directory traversal should still be blocked for: {}",
                file.name()
            );
        }
    }
}
