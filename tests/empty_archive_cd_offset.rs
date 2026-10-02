//! An empty archive's end-of-central-directory record must not place the central directory past
//! itself. Such a record used to be accepted, and `ZipWriter::new_append` then wrote the next
//! entry at the declared offset: a 22-byte input grew to 2 GiB (or 4 GiB).

use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// A 22-byte EOCD with no entries, an empty central directory and the given CD offset.
fn empty_eocd(cd_offset: u32) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"PK\x05\x06");
    v.extend_from_slice(&[0; 8]); // disk numbers and entry counts
    v.extend_from_slice(&0u32.to_le_bytes()); // central directory size
    v.extend_from_slice(&cd_offset.to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes()); // comment length
    v
}

#[test]
fn empty_archive_with_cd_offset_past_eocd_is_rejected() {
    for offset in [23, 0x7fff_ffff, 0xffff_ff00] {
        let data = empty_eocd(offset);
        assert!(
            ZipArchive::new(Cursor::new(&data[..])).is_err(),
            "CD offset {offset:#x} in a {}-byte file was accepted",
            data.len()
        );
        assert!(
            ZipWriter::new_append(Cursor::new(data)).is_err(),
            "new_append accepted CD offset {offset:#x}"
        );
    }
}

#[test]
fn empty_archive_still_opens_and_appends() {
    // The canonical empty archive (offset 0) and one with a prefix (the offset is then relative
    // to the start of the archive, and the EOCD sits further in).
    for prefix in [&b""[..], b"#!/bin/sh\nexit 0\n"] {
        let mut data = prefix.to_vec();
        data.extend_from_slice(&empty_eocd(0));
        let archive = ZipArchive::new(Cursor::new(&data[..])).expect("empty archive opens");
        assert_eq!(archive.len(), 0);

        let mut w = ZipWriter::new_append(Cursor::new(data.clone())).expect("new_append");
        // Stored: reading back must not depend on a decompressor (zopfli-only builds can't inflate).
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        w.start_file("a.txt", opts).unwrap();
        w.write_all(b"hello").unwrap();
        let out = w.finish().unwrap().into_inner();
        assert!(
            out.len() < 1024,
            "appending one small entry gave {} bytes",
            out.len()
        );
        let mut archive = ZipArchive::new(Cursor::new(out)).unwrap();
        let mut s = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("a.txt").unwrap(), &mut s).unwrap();
        assert_eq!(s, "hello");
    }
}
