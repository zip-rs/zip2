use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

#[cfg(not(miri))]
const LARGE_PAYLOAD_SIZE: usize = 1024 * 1024;
#[cfg(miri)]
const LARGE_PAYLOAD_SIZE: usize = 1024 * 64;

#[cfg(not(miri))]
const MANY_FILES: usize = 2000;
#[cfg(miri)]
const MANY_FILES: usize = 20;

/// A reader wrapper that counts the number of bytes read.
struct TrackingReader<R> {
    inner: R,
    bytes_read: usize,
    read_calls: usize,
}

impl<R> TrackingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            bytes_read: 0,
            read_calls: 0,
        }
    }
}

impl<R: Read> Read for TrackingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes_read += n;
        self.read_calls += 1;
        Ok(n)
    }
}

impl<R: Seek> Seek for TrackingReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

#[test]
fn test_verify_issue_1141_unnecessary_full_file_scan() {
    // Create an archive with 1 MB of payload data
    let mut zip_data = Vec::new();
    {
        let mut writer = ZipWriter::new(Cursor::new(&mut zip_data));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file("large_payload.bin", opts).unwrap();
        // 1 MB payload (1,048,576 bytes)
        let chunk = vec![0xAA; LARGE_PAYLOAD_SIZE];
        writer.write_all(&chunk).unwrap();
        writer.finish().unwrap();
    }

    let archive_byte_size = zip_data.len();
    assert!(archive_byte_size > LARGE_PAYLOAD_SIZE);

    // Now open the archive with TrackingReader
    let mut tracker = TrackingReader::new(Cursor::new(zip_data));
    let archive = ZipArchive::new(&mut tracker).unwrap();
    assert_eq!(archive.len(), 1);

    println!(
        "Archive size: {} bytes. Bytes read during ZipArchive::new: {} bytes (read calls: {})",
        archive_byte_size, tracker.bytes_read, tracker.read_calls
    );

    // If the bug exists, tracker.bytes_read will be approximately equal to archive_byte_size (over 1 MB!)
    // If fixed, tracker.bytes_read should only be the Central Directory and EOCD (less than 10 KB).
    assert!(
        tracker.bytes_read < 10_000,
        "BUG REPRODUCED: ZipArchive::new read {} bytes out of {} total bytes!",
        tracker.bytes_read,
        archive_byte_size
    );
}

#[test]
fn test_verify_issue_1141_zip64_unnecessary_full_file_scan() {
    let mut zip_data = Vec::new();
    {
        let mut writer = ZipWriter::new(Cursor::new(&mut zip_data));
        let opts = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .large_file(true);
        writer.start_file("large_payload_zip64.bin", opts).unwrap();
        let chunk = vec![0xBB; LARGE_PAYLOAD_SIZE]; // 1 MB payload
        writer.write_all(&chunk).unwrap();
        writer.finish().unwrap();
    }

    let archive_byte_size = zip_data.len();
    assert!(archive_byte_size > LARGE_PAYLOAD_SIZE);

    let mut tracker = TrackingReader::new(Cursor::new(zip_data));
    let archive = ZipArchive::new(&mut tracker).unwrap();
    assert_eq!(archive.len(), 1);

    println!(
        "[ZIP64] Archive size: {} bytes. Bytes read during ZipArchive::new: {} bytes (read calls: {})",
        archive_byte_size, tracker.bytes_read, tracker.read_calls
    );

    assert!(
        tracker.bytes_read < 10_000,
        "BUG REPRODUCED ON ZIP64: ZipArchive::new read {} bytes out of {} total bytes!",
        tracker.bytes_read,
        archive_byte_size
    );
}

#[test]
fn test_zip64_archive_with_prefix_detect_offset() {
    let mut zip_data = Vec::new();
    {
        let mut writer = ZipWriter::new(Cursor::new(&mut zip_data));
        let opts = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .large_file(true);
        writer.start_file("prefixed.bin", opts).unwrap();
        let chunk = vec![0xCC; LARGE_PAYLOAD_SIZE]; // 1 MB payload
        writer.write_all(&chunk).unwrap();
        writer.finish().unwrap();
    }

    let mut prefixed_data = vec![0x7F; 512];
    prefixed_data.extend_from_slice(&zip_data);

    let mut tracker = TrackingReader::new(Cursor::new(prefixed_data));
    let archive = ZipArchive::new(&mut tracker).unwrap();
    assert_eq!(archive.len(), 1);
    assert_eq!(archive.offset(), 512);
    drop(archive);
    println!(
        "[Prefixed ZIP64] Bytes read: {} across {} calls",
        tracker.bytes_read, tracker.read_calls
    );
}

#[test]
fn test_many_files_central_directory_parsing() {
    let mut zip_data = Vec::new();
    {
        let mut writer = ZipWriter::new(Cursor::new(&mut zip_data));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for i in 0..MANY_FILES {
            let name = format!("entry_{i}.txt");
            writer.start_file(name, opts).unwrap();
            writer.write_all(b"x").unwrap();
        }
        writer.finish().unwrap();
    }

    let mut tracker = TrackingReader::new(Cursor::new(zip_data));
    let archive = ZipArchive::new(&mut tracker).unwrap();
    assert_eq!(archive.len(), MANY_FILES);
}

#[test]
fn test_non_abutting_central_directory_does_not_scan_payload() {
    let mut zip_data = Vec::new();
    {
        let mut writer = ZipWriter::new(Cursor::new(&mut zip_data));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file("large_payload.bin", opts).unwrap();
        let chunk = vec![0xDD; LARGE_PAYLOAD_SIZE]; // 1 MB payload
        writer.write_all(&chunk).unwrap();
        writer.finish().unwrap();
    }

    // Insert 64 bytes of gap between Central Directory and EOCD
    let eocd_start = zip_data.len() - 22;
    let mut non_abutting_data = Vec::new();
    non_abutting_data.extend_from_slice(&zip_data[..eocd_start]);
    non_abutting_data.extend_from_slice(&[0xEE; 64]); // 64 bytes gap
    non_abutting_data.extend_from_slice(&zip_data[eocd_start..]);

    let mut tracker = TrackingReader::new(Cursor::new(non_abutting_data));
    let archive = ZipArchive::new(&mut tracker).unwrap();
    assert_eq!(archive.len(), 1);
    drop(archive);

    println!(
        "[Non-Abutting ZIP] Bytes read: {} across {} calls",
        tracker.bytes_read, tracker.read_calls
    );

    assert!(
        tracker.bytes_read < 10_000,
        "BYPASS CONFIRMED: Non-abutting CD forced full-file scan of {} bytes!",
        tracker.bytes_read
    );
}
