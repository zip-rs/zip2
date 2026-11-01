use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

struct BoundedReadCursor(Cursor<Vec<u8>>);

impl Read for BoundedReadCursor {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.len() > 64 * 1024 {
            return Err(io::Error::other(format!(
                "read buffer exceeds 64 KiB: {} bytes",
                buf.len()
            )));
        }
        self.0.read(buf)
    }
}

impl Write for BoundedReadCursor {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Seek for BoundedReadCursor {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.0.seek(pos)
    }
}

#[test]
fn deep_copy_uses_bounded_read_buffer() {
    #[cfg(not(miri))]
    const CONTENTS_LEN: usize = 256 * 1024;

    #[cfg(miri)]
    const CONTENTS_LEN: usize = 16 * 1024;

    let contents = vec![0x5a; CONTENTS_LEN];
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "original",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(&contents).unwrap();
    let archive = writer.finish().unwrap().into_inner();

    let mut writer = ZipWriter::new_append(BoundedReadCursor(Cursor::new(archive))).unwrap();
    writer.deep_copy_file("original", "copy").unwrap();
    let archive = writer.finish().unwrap().0.into_inner();

    let mut reader = ZipArchive::new(Cursor::new(archive)).unwrap();
    let mut copy = Vec::new();
    reader
        .by_name("copy")
        .unwrap()
        .read_to_end(&mut copy)
        .unwrap();
    assert_eq!(copy, contents);
}

use zip::ZipUpdate;
