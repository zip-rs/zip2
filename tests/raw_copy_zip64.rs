//! A raw copy of an entry with a ZIP64 extended information field (0x0001) must not end up with
//! two of them.
//!
//! `ZipWriter::raw_copy_file` passes every parsed extra field into the new entry's options as
//! raw bytes, the source's ZIP64 field included. When the copy needs a ZIP64 field of its own
//! (its sizes or its offset pass 4 GiB), the writer adds one, and the entry carries both. Readers
//! then take the wrong values: in a CBZ over 4 GiB, this crate's own reader failed on the copy
//! with "Invalid local file header".

use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

use zip::extra_fields::ExtraField;
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::{CompressionMethod, ZipArchive};

/// Where the copied archive starts, so its entries' offsets need ZIP64 without writing 4 GiB.
const BASE: u64 = 1 << 32;

/// A file whose first `BASE` bytes are zeros that are never stored; the rest is in memory.
#[derive(Default)]
struct Shifted {
    data: Vec<u8>,
    pos: u64,
}

impl Write for Shifted {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Some(at) = self.pos.checked_sub(BASE) else {
            return Err(io::Error::other("write into the unstored zeros"));
        };
        let (at, end) = (at as usize, at as usize + buf.len());
        if self.data.len() < end {
            self.data.resize(end, 0);
        }
        self.data[at..end].copy_from_slice(buf);
        self.pos += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for Shifted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = if self.pos < BASE {
            let n = buf.len().min((BASE - self.pos) as usize);
            buf[..n].fill(0);
            n
        } else {
            let at = ((self.pos - BASE) as usize).min(self.data.len());
            let n = buf.len().min(self.data.len() - at);
            buf[..n].copy_from_slice(&self.data[at..at + n]);
            n
        };
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Shifted {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::End(delta) => (BASE + self.data.len() as u64).checked_add_signed(delta),
            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
        };
        self.pos = target.ok_or_else(|| io::Error::other("seek before the start"))?;
        Ok(self.pos)
    }
}

/// "small.bin" is written with `large_file(true)`, so its central header has a ZIP64 field;
/// "after.txt" follows it.
fn source_archive() -> Vec<u8> {
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file("small.bin", stored.large_file(true))
        .unwrap();
    writer.write_all(b"small, but marked large").unwrap();
    writer.start_file("after.txt", stored).unwrap();
    writer.write_all(b"after").unwrap();
    writer.finish().unwrap().into_inner()
}

fn zip64_fields<R: Read + Seek>(archive: &mut ZipArchive<R>, name: &str) -> usize {
    archive
        .by_name(name)
        .unwrap()
        .extra_data_fields()
        .filter(|field| matches!(field, ExtraField::Zip64ExtendedInformation(_)))
        .count()
}

#[test]
fn raw_copy_past_4_gib_writes_one_zip64_field_per_entry() {
    let mut source = ZipArchive::new(Cursor::new(source_archive())).unwrap();
    assert_eq!(zip64_fields(&mut source, "small.bin"), 1);

    let shifted = Shifted {
        pos: BASE,
        ..Shifted::default()
    };
    let mut writer = ZipWriter::new(shifted);
    for i in 0..source.len() {
        writer
            .raw_copy_file(source.by_index_raw(i).unwrap())
            .unwrap();
    }
    let copy = writer.finish().unwrap();

    let mut copy = ZipArchive::new(copy).unwrap();
    for (name, contents) in [
        ("small.bin", &b"small, but marked large"[..]),
        ("after.txt", b"after"),
    ] {
        assert!(copy.by_name(name).unwrap().header_start() >= BASE);
        assert_eq!(zip64_fields(&mut copy, name), 1, "{name}'s ZIP64 fields");
        let mut read = Vec::new();
        copy.by_name(name).unwrap().read_to_end(&mut read).unwrap();
        assert_eq!(read, contents, "{name}");
    }
}
