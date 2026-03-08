use std::{fs::File, io::Write};
use tempfile::tempdir;
use zip::{ZipWriter, write::SimpleFileOptions};

fn write_data(w: &mut dyn Write, size: usize) {
    let chunks = 1 << 20; // 1MB chunks
    let mut written = 0;
    let buf = vec![0x21; chunks];
    while written < size {
        let to_write = (size - written).min(chunks);
        w.write_all(&buf[..to_write]).unwrap();
        written += to_write;
    }
}

#[test]
fn test_append_near_4gb() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("large-then-small.zip");

    // Create a new zip file with a large file close to 4GB
    {
        let file = File::create(&path).unwrap();
        let mut writer = ZipWriter::new(file);

        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        writer.start_file_from_path("close_to_4gb", opts).unwrap();

        // Write a file that's just under 4GB (4GB - 1 byte)
        let size = u32::MAX;
        write_data(&mut writer, size as usize);

        // Add a small file
        writer.start_file_from_path("small_file", opts).unwrap();
        write_data(&mut writer, 1024);

        writer.finish().unwrap();
    }

    // Now append to the zip file
    {
        let file = File::options().read(true).write(true).open(&path).unwrap();
        let mut writer = ZipWriter::new_append(file).unwrap();

        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        // Add another small file
        writer.start_file_from_path("appended_file", opts).unwrap();
        write_data(&mut writer, 1024);

        writer.finish().unwrap();
    }

    // Verify the zip file is valid by reading it
    {
        let file = File::open(&path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();

        assert_eq!(archive.len(), 3);
        assert!(!archive.has_overlapping_files().unwrap());
        assert!(archive.file_names().any(|name| name == "close_to_4gb"));
        assert!(archive.file_names().any(|name| name == "small_file"));
        assert!(archive.file_names().any(|name| name == "appended_file"));
    }
}

#[test]
fn test_append_near_4gb_with_1gb_files() {
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
            write_data(&mut writer, size as usize);
        }

        // Add a small file
        writer.start_file_from_path("small_file", opts).unwrap();
        write_data(&mut writer, 1024);

        writer.finish().unwrap();
    }

    // Now append to the zip file
    {
        let file = File::options().read(true).write(true).open(&path).unwrap();
        let mut writer = ZipWriter::new_append(file).unwrap();

        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        // Add another small file
        writer.start_file_from_path("appended_file", opts).unwrap();
        write_data(&mut writer, 1024);

        writer.finish().unwrap();
    }

    // Verify the zip file is valid by reading it
    {
        let file = File::open(&path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();

        assert_eq!(archive.len(), 6);
        assert!(!archive.has_overlapping_files().unwrap());
        assert!(archive.file_names().any(|name| name == "close_to_4gb_0"));
        assert!(archive.file_names().any(|name| name == "close_to_4gb_1"));
        assert!(archive.file_names().any(|name| name == "close_to_4gb_2"));
        assert!(archive.file_names().any(|name| name == "close_to_4gb_3"));
        assert!(archive.file_names().any(|name| name == "small_file"));
        assert!(archive.file_names().any(|name| name == "appended_file"));
    }
}

// A smaller test that doesn't create a 4GB file but still tests the logic
#[test]
fn test_append_with_large_file_flag() {
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
        write_data(&mut writer, 1024);

        writer.finish().unwrap();
    }

    // Now append to the zip file
    {
        let file = File::options().read(true).write(true).open(&path).unwrap();
        let mut writer = ZipWriter::new_append(file).unwrap();

        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        // Add another file
        writer.start_file_from_path("file2", opts).unwrap();
        write_data(&mut writer, 1024);

        writer.finish().unwrap();
    }

    // Verify the zip file is valid by reading it
    {
        let file = File::open(&path).unwrap();
        let archive = zip::ZipArchive::new(file).unwrap();

        assert_eq!(archive.len(), 2);
        assert!(archive.file_names().any(|name| name == "file1"));
        assert!(archive.file_names().any(|name| name == "file2"));
    }
}
