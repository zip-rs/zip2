//! Tests related to big zip file

// Only on little endian because we cannot use fs with miri CI
#![cfg(all(target_endian = "little", not(miri)))]


fn write_data(w: &mut dyn std::io::Write, size: usize) -> Result<(), std::io::Error> {
    let chunks = 1 << 20; // 1MB chunks
    let mut written = 0;
    let buf = vec![0x21; chunks];
    while written < size {
        let to_write = (size - written).min(chunks);
        w.write_all(&buf[..to_write])?;
        written += to_write;
    }
    Ok(())
}

#[test]
fn test_append_4gb_without_large_file() {
    use std::fs::File;
    use tempfile::tempdir;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let dir = tempdir().unwrap();
    let path = dir
        .path()
        .join("debug_large_without_large_file_options.zip");
    //let path = std::path::PathBuf::from("debug_large_without_large_file_options.zip");

    let file = File::create(&path).unwrap();
    let mut writer = ZipWriter::new(file);

    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    writer.start_file_from_path("4gb_file", opts).unwrap();

    // Write a file that's 4GB
    let size = u32::MAX;
    let write_result = write_data(&mut writer, size as usize); // check is error

    assert!(write_result.is_err());
}

///  We cannot run this test because on wasm32 we cannot fit the u32:MAX in the usize
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_append_4gb_with_large_file() {
    use std::fs::File;
    use std::io::Read;
    use tempfile::tempdir;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let dir = tempdir().unwrap();
    let path = dir.path().join("debug_large_with_large_file_options.zip");
    //let path = std::path::PathBuf::from("debug_large_with_large_file_options.zip");

    let zipfile = File::create(&path).unwrap();
    let mut writer = ZipWriter::new(zipfile);

    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);

    writer.start_file_from_path("4gb_file", opts).unwrap();

    // Write a file that's 4GB
    let size = u32::MAX;
    let write_result = write_data(&mut writer, size as usize); // check is error

    assert!(write_result.is_ok());
    let mut zip = writer.finish_into_readable().unwrap();
    let file_res = zip.by_name("4gb_file");
    assert!(file_res.is_ok());
    let file = file_res.unwrap();
    eprintln!("{file:?}");

    let mut file = File::open(&path).unwrap();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();

    // local header
    assert_eq!(buffer[18..22], [0xFF, 0xFF, 0xFF, 0xFF]);
    assert_eq!(buffer[22..26], [0xFF, 0xFF, 0xFF, 0xFF]);

    // extra field of local header
    let extra_field_start = 38;
    assert_eq!(buffer[extra_field_start..40], [0x01, 0x00]);
    assert_eq!(buffer[40..42], [16, 0x00]);
    assert_eq!(
        buffer[42..50],
        [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00]
    );
    assert_eq!(
        buffer[50..58],
        [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00]
    );

    // extra fields of central header
    let start = extra_field_start + 20 + u32::MAX as usize + 54;
    assert_eq!(buffer[start..(start + 2)], [0x01, 0x00]);
    assert_eq!(buffer[(start + 2)..(start + 4)], [16, 0x00]);
    assert_eq!(
        buffer[(start + 4)..(start + 12)],
        [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00]
    );
    assert_eq!(
        buffer[(start + 12)..(start + 20)],
        [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00]
    );
}

#[test]
fn test_append_near_4gb() {
    use std::fs::File;
    use tempfile::tempdir;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let dir = tempdir().unwrap();
    let path = dir.path().join("large-then-small.zip");

    // Create a new zip file with a large file close to 4GB
    {
        let file = File::create(&path).unwrap();
        let mut writer = ZipWriter::new(file);

        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        writer.start_file_from_path("close_to_4gb", opts).unwrap();

        // Write a file that's just under 4GB (4GB - 1 byte)
        let size = u32::MAX - 1;
        write_data(&mut writer, size as usize).unwrap();

        // Add a small file
        writer.start_file_from_path("small_file", opts).unwrap();
        write_data(&mut writer, 1024).unwrap();

        writer.finish().unwrap();
    }

    // Now append to the zip file
    {
        let file = File::options().read(true).write(true).open(&path).unwrap();
        let mut writer = ZipWriter::new_append(file).unwrap();

        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        // Add another small file
        writer.start_file_from_path("appended_file", opts).unwrap();
        write_data(&mut writer, 1024).unwrap();

        writer.finish().unwrap();
    }

    // Verify the zip file is valid by reading it
    {
        let file = File::open(&path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();

        assert_eq!(archive.len(), 3);
        assert!(!archive.has_overlapping_files().unwrap());
        assert!(
            archive
                .file_names()
                .any(|name| name.unwrap() == "close_to_4gb")
        );
        assert!(
            archive
                .file_names()
                .any(|name| name.unwrap() == "small_file")
        );
        assert!(
            archive
                .file_names()
                .any(|name| name.unwrap() == "appended_file")
        );
    }
}

#[test]
fn test_append_near_4gb_with_1gb_files() {
    use std::fs::File;
    use tempfile::tempdir;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let dir = tempdir().unwrap();
    let path = dir.path().join("large-then-small.zip");

    // Create a new zip file with 4 files totaling 1GB
    {
        let file = File::create(&path).unwrap();
        let mut writer = ZipWriter::new(file);

        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        for i in 0..=3 {
            writer
                .start_file_from_path(format!("close_to_4gb_{i}"), opts)
                .unwrap();

            // Write a file that's 1 GB
            let size = 1u64 << 30;
            write_data(&mut writer, size as usize).unwrap();
        }

        // Add a small file
        writer.start_file_from_path("small_file", opts).unwrap();
        write_data(&mut writer, 1024).unwrap();

        writer.finish().unwrap();
    }

    // Now append to the zip file
    {
        let file = File::options().read(true).write(true).open(&path).unwrap();
        let mut writer = ZipWriter::new_append(file).unwrap();

        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        // Add another small file
        writer.start_file_from_path("appended_file", opts).unwrap();
        write_data(&mut writer, 1024).unwrap();

        writer.finish().unwrap();
    }

    // Verify the zip file is valid by reading it
    {
        let file = File::open(&path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();

        assert_eq!(archive.len(), 6);
        assert!(!archive.has_overlapping_files().unwrap());
        assert!(
            archive
                .file_names()
                .any(|name| name.unwrap() == "close_to_4gb_0")
        );
        assert!(
            archive
                .file_names()
                .any(|name| name.unwrap() == "close_to_4gb_1")
        );
        assert!(
            archive
                .file_names()
                .any(|name| name.unwrap() == "close_to_4gb_2")
        );
        assert!(
            archive
                .file_names()
                .any(|name| name.unwrap() == "close_to_4gb_3")
        );
        assert!(
            archive
                .file_names()
                .any(|name| name.unwrap() == "small_file")
        );
        assert!(
            archive
                .file_names()
                .any(|name| name.unwrap() == "appended_file")
        );
    }
}

// A smaller test that doesn't create a 4GB file but still tests the logic
#[test]
fn test_append_with_large_file_flag() {
    use std::fs::File;
    use tempfile::tempdir;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let dir = tempdir().unwrap();
    let path = dir.path().join("test.zip");

    // Create a new zip file
    {
        let file = File::create(&path).unwrap();
        let mut writer = ZipWriter::new(file);

        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .large_file(true); // Force ZIP64 format

        writer.start_file_from_path("file1", opts).unwrap();
        write_data(&mut writer, 1024).unwrap();

        writer.finish().unwrap();
    }

    // Now append to the zip file
    {
        let file = File::options().read(true).write(true).open(&path).unwrap();
        let mut writer = ZipWriter::new_append(file).unwrap();

        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        // Add another file
        writer.start_file_from_path("file2", opts).unwrap();
        write_data(&mut writer, 1024).unwrap();

        writer.finish().unwrap();
    }

    // Verify the zip file is valid by reading it
    {
        let file = File::open(&path).unwrap();
        let archive = zip::ZipArchive::new(file).unwrap();

        assert_eq!(archive.len(), 2);
        assert!(archive.file_names().any(|name| name.unwrap() == "file1"));
        assert!(archive.file_names().any(|name| name.unwrap() == "file2"));
    }
}
