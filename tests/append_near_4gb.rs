//! Tests related to big zip file

// FIXME: 4 of these 5 tests don't use the filesystem, but they're too slow on Miri because of the
// rposition call in write_to_sparse_vec.
#![cfg(not(miri))]

use std::io::{self, Read, Seek, SeekFrom, Write};

const DENSE_SEGMENT_START_INTERVAL: u64 = (1 << 30) - 1024;
const SEGMENT_COUNT: usize = 5;

fn as_usize_clamped(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// A cursor that stores nonzero bytes in 1 GiB segments and omits zero-filled ranges.
struct AddressHoleCursor {
    segments: [Vec<u8>; SEGMENT_COUNT],
    position: u64,
    len: u64,
}

impl AddressHoleCursor {
    fn new_at(position: u64) -> Self {
        Self {
            segments: std::array::from_fn(|_| Vec::new()),
            position,
            len: 0,
        }
    }

    fn read_from_sparse_vec(vec: &[u8], offset: usize, buf: &mut [u8]) {
        let copied = vec.len().saturating_sub(offset).min(buf.len());
        if copied > 0 {
            buf[..copied].copy_from_slice(&vec[offset..offset + copied]);
        }
        buf[copied..].fill(0);
    }

    fn write_to_sparse_vec(vec: &mut Vec<u8>, offset: usize, buf: &[u8]) {
        let existing = vec.len().saturating_sub(offset).min(buf.len());
        if existing > 0 {
            vec[offset..offset + existing].copy_from_slice(&buf[..existing]);
        }

        if let Some(last_nonzero) = buf.iter().rposition(|byte| *byte != 0) {
            let end = offset + last_nonzero + 1;
            vec.resize(vec.len().max(end), 0);
            vec[offset..end].copy_from_slice(&buf[..=last_nonzero]);
        }
    }
}

impl Read for AddressHoleCursor {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut read = 0;

        while read < buf.len() && self.position < self.len {
            let segment = as_usize_clamped(self.position / DENSE_SEGMENT_START_INTERVAL);
            let segment_offset = self.position % DENSE_SEGMENT_START_INTERVAL;
            let count = (buf.len() - read)
                .min(as_usize_clamped(
                    DENSE_SEGMENT_START_INTERVAL - segment_offset,
                ))
                .min(as_usize_clamped(self.len - self.position));
            let vec = &self.segments[segment];
            let offset = as_usize_clamped(segment_offset);
            Self::read_from_sparse_vec(vec, offset, &mut buf[read..read + count]);
            self.position += count as u64;
            read += count;
        }

        Ok(read)
    }
}

impl Write for AddressHoleCursor {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut written = 0;

        while written < buf.len() {
            let segment = as_usize_clamped(self.position / DENSE_SEGMENT_START_INTERVAL);
            let segment_offset = self.position % DENSE_SEGMENT_START_INTERVAL;
            let count = (buf.len() - written).min(as_usize_clamped(
                DENSE_SEGMENT_START_INTERVAL - segment_offset,
            ));
            let vec = &mut self.segments[segment];
            let offset = as_usize_clamped(segment_offset);
            Self::write_to_sparse_vec(vec, offset, &buf[written..written + count]);
            self.position += count as u64;
            written += count;
        }

        self.len = self.len.max(self.position);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for AddressHoleCursor {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.len) + i128::from(offset),
        };
        if !(0..=i128::from(u64::MAX)).contains(&position) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid seek to a negative or overflowing position",
            ));
        }
        self.position = position as u64;
        Ok(self.position)
    }
}

fn read_at<R: Read + Seek>(reader: &mut R, position: u64, buf: &mut [u8]) {
    reader.seek(SeekFrom::Start(position)).unwrap();
    reader.read_exact(buf).unwrap();
}

fn write_data(w: &mut impl Write, size: usize) -> Result<(), io::Error> {
    let chunks = 1 << 20; // 1MB chunks
    let mut written = 0;
    let buf = vec![0; chunks];
    while written < size {
        let to_write = (size - written).min(chunks);
        w.write_all(&buf[..to_write])?;
        written += to_write;
    }
    Ok(())
}

#[test]
fn test_append_4gb_without_large_file() {
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let mut writer = ZipWriter::new(AddressHoleCursor::new_at(0));

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
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let mut writer = ZipWriter::new(AddressHoleCursor::new_at(0));

    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);

    writer.start_file_from_path("4gb_file", opts).unwrap();

    // Write a file that's 4GB
    let size = u32::MAX;
    let write_result = write_data(&mut writer, size as usize); // check is error

    assert!(write_result.is_ok());
    let mut zip = writer.finish().unwrap();
    let mut local_header = [0; 58];
    read_at(&mut zip, 0, &mut local_header);

    // local header
    assert_eq!(local_header[18..22], [0xFF, 0xFF, 0xFF, 0xFF]);
    assert_eq!(local_header[22..26], [0xFF, 0xFF, 0xFF, 0xFF]);

    // extra field of local header
    let extra_field_start = 38;
    assert_eq!(local_header[extra_field_start..40], [0x01, 0x00]);
    assert_eq!(local_header[40..42], [16, 0x00]);
    assert_eq!(
        local_header[42..50],
        [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00]
    );
    assert_eq!(
        local_header[50..58],
        [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00]
    );

    // extra field of central header
    let end = zip.seek(SeekFrom::End(0)).unwrap();
    let mut tail = [0; 256];
    read_at(&mut zip, end - tail.len() as u64, &mut tail);
    let central_header = tail
        .windows(4)
        .rposition(|bytes| bytes == [0x50, 0x4b, 0x01, 0x02])
        .unwrap();
    let extra_field_start = central_header + 54;
    assert_eq!(tail[extra_field_start..extra_field_start + 2], [0x01, 0x00]);
    assert_eq!(
        tail[extra_field_start + 2..extra_field_start + 4],
        [16, 0x00]
    );
    assert_eq!(
        tail[extra_field_start + 4..extra_field_start + 12],
        [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00]
    );
    assert_eq!(
        tail[extra_field_start + 12..extra_field_start + 20],
        [0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00]
    );

    let mut archive = zip::ZipArchive::new(zip).unwrap();
    let file = archive.by_name("4gb_file").unwrap();
    assert_eq!(file.size(), u32::MAX as u64);
}

#[test]
fn test_append_near_4gb() {
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    // Create a new zip file with a large file close to 4GB
    let cursor = {
        let mut writer = ZipWriter::new(AddressHoleCursor::new_at(0));

        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        writer.start_file_from_path("close_to_4gb", opts).unwrap();

        // The zero-filled body takes the archive close to 4 GiB without allocating it.
        write_data(&mut writer, u32::MAX as usize - 1024).unwrap();

        // Add a small file
        writer.start_file_from_path("small_file", opts).unwrap();
        write_data(&mut writer, 1024).unwrap();

        writer.finish().unwrap()
    };

    // Now append to the zip file
    let cursor = {
        let mut writer = ZipWriter::new_append(cursor).unwrap();

        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        // Add another small file
        writer.start_file_from_path("appended_file", opts).unwrap();
        write_data(&mut writer, 1024).unwrap();

        writer.finish().unwrap()
    };

    // Verify the zip file is valid by reading it
    {
        let mut archive = zip::ZipArchive::new(cursor).unwrap();

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
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    // Create a new zip file with 4 files totaling 1GB
    let cursor = {
        let mut writer = ZipWriter::new(AddressHoleCursor::new_at(0));

        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        for i in 0..=3 {
            writer
                .start_file_from_path(format!("close_to_4gb_{i}"), opts)
                .unwrap();

            // Zero-filled payloads exercise large offsets without allocating their bodies.
            write_data(&mut writer, 1 << 30).unwrap();
        }

        // Add a small file
        writer.start_file_from_path("small_file", opts).unwrap();
        write_data(&mut writer, 1024).unwrap();

        writer.finish().unwrap()
    };

    // Now append to the zip file
    let cursor = {
        let mut writer = ZipWriter::new_append(cursor).unwrap();

        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        // Add another small file
        writer.start_file_from_path("appended_file", opts).unwrap();
        write_data(&mut writer, 1024).unwrap();

        writer.finish().unwrap()
    };

    // Verify the zip file is valid by reading it
    {
        let mut archive = zip::ZipArchive::new(cursor).unwrap();

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
