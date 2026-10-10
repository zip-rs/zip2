//! A ZIP64 extended information field in a central directory header carries only the values whose
//! fixed-size field in that header holds the 0xFFFFFFFF sentinel, in the order uncompressed size,
//! compressed size, local header offset (APPNOTE 4.5.3). The writer used to add both sizes to any
//! ZIP64 field it found on an entry, so an entry whose offset alone needed ZIP64 got a field with
//! three values but one sentinel: a reader that follows the spec takes the first value (a size)
//! as the offset. It also made the second write of the same central directory 16 bytes longer
//! than the first, which `finish` relies on being the same size.

use std::io::{Cursor, Read, Write};

use zip::write::{ExtendedFileOptions, FileOptions, SimpleFileOptions};
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const ZIP64_ID: u16 = 0x0001;

/// An archive with one stored entry `a.txt` ("hello") whose central header offset field is
/// 0xFFFFFFFF with no ZIP64 field, preceded by `prefix_len` bytes. zip2 reads that entry's offset
/// as 0xFFFFFFFF plus the archive offset, so writing its central header back needs a ZIP64 field
/// for the offset only: its sizes fit in 32 bits.
fn archive_with_sentinel_offset(prefix_len: usize) -> Vec<u8> {
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    w.start_file("a.txt", opts).unwrap();
    w.write_all(b"hello").unwrap();
    let mut zip = w.finish().unwrap().into_inner();

    // Central header of the only entry: set its local header offset (at +42) to the sentinel.
    let cdh = zip
        .windows(4)
        .position(|s| s == b"PK\x01\x02")
        .expect("central directory header");
    zip[cdh + 42..cdh + 46].copy_from_slice(&u32::MAX.to_le_bytes());

    let mut out = vec![b'#'; prefix_len];
    out.extend_from_slice(&zip);
    out
}

/// Every central directory header in `archive`: (name, size/offset fields holding 0xFFFFFFFF in
/// spec order, number of u64 values in its ZIP64 field). The central directory is found by
/// signature after the EOCD's offset field (which is relative to the archive start).
fn central_headers(archive: &[u8]) -> Vec<(String, usize, usize)> {
    let eocd = archive
        .windows(4)
        .rposition(|s| s == b"PK\x05\x06")
        .expect("end of central directory record");
    let entries = u16::from_le_bytes(archive[eocd + 10..eocd + 12].try_into().unwrap());
    let rel = u32::from_le_bytes(archive[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
    let mut at = rel
        + archive[rel..]
            .windows(4)
            .position(|s| s == b"PK\x01\x02")
            .expect("central directory");
    let mut headers = Vec::new();
    for _ in 0..entries {
        assert_eq!(
            &archive[at..at + 4],
            b"PK\x01\x02",
            "central directory header"
        );
        let u16_at = |o: usize| u16::from_le_bytes(archive[at + o..at + o + 2].try_into().unwrap());
        let u32_at = |o: usize| u32::from_le_bytes(archive[at + o..at + o + 4].try_into().unwrap());
        // compressed size +20, uncompressed size +24, offset +42; spec order: uncompressed,
        // compressed, offset.
        let sentinels = [u32_at(24), u32_at(20), u32_at(42)]
            .iter()
            .filter(|&&v| v == u32::MAX)
            .count();
        let (name_len, extra_len, comment_len) = (
            u16_at(28) as usize,
            u16_at(30) as usize,
            u16_at(32) as usize,
        );
        let name = String::from_utf8_lossy(&archive[at + 46..at + 46 + name_len]).into_owned();
        let extra = &archive[at + 46 + name_len..at + 46 + name_len + extra_len];
        let mut values = 0;
        let mut i = 0;
        while i + 4 <= extra.len() {
            let id = u16::from_le_bytes(extra[i..i + 2].try_into().unwrap());
            let len = u16::from_le_bytes(extra[i + 2..i + 4].try_into().unwrap()) as usize;
            if id == ZIP64_ID {
                values = len / 8;
            }
            i += 4 + len;
        }
        headers.push((name, sentinels, values));
        at += 46 + name_len + extra_len + comment_len;
    }
    headers
}

/// A spec-valid archive with one stored entry `a.txt` ("hello") whose local header offset (0) is
/// stored in a ZIP64 field: the central header's offset field holds the sentinel and its ZIP64
/// field holds exactly that one value. Sizes are small and stored in the header itself.
fn archive_with_zip64_offset() -> Vec<u8> {
    // Built with a placeholder id, then renamed to 0x0001: the writer refuses custom fields under
    // the ZIP64 id. Central-only, so the local header is untouched.
    const PLACEHOLDER_ID: u16 = 0x7a7a;
    let mut opts: FileOptions<ExtendedFileOptions> =
        FileOptions::default().compression_method(CompressionMethod::Stored);
    opts.add_extra_field(PLACEHOLDER_ID, 0u64.to_le_bytes(), true)
        .expect("extra field should be accepted");
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    w.start_file("a.txt", opts).unwrap();
    w.write_all(b"hello").unwrap();
    let mut zip = w.finish().unwrap().into_inner();

    let cdh = zip
        .windows(4)
        .position(|s| s == b"PK\x01\x02")
        .expect("central directory header");
    let field = zip[cdh..]
        .windows(4)
        .position(|s| s[..2] == PLACEHOLDER_ID.to_le_bytes() && s[2..] == 8u16.to_le_bytes())
        .expect("placeholder field in the central header")
        + cdh;
    zip[field..field + 2].copy_from_slice(&ZIP64_ID.to_le_bytes());
    zip[cdh + 42..cdh + 46].copy_from_slice(&u32::MAX.to_le_bytes());
    zip
}

fn append_one(archive: Vec<u8>) -> Vec<u8> {
    let mut w = ZipWriter::new_append(Cursor::new(archive)).expect("new_append");
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    w.start_file("b.txt", opts).unwrap();
    w.write_all(b"world").unwrap();
    w.finish().expect("finish").into_inner()
}

#[test]
fn zip64_field_holds_only_the_values_the_header_marks() {
    let input = archive_with_zip64_offset();
    // The fixture itself is well formed: one sentinel, one value.
    assert_eq!(central_headers(&input), [("a.txt".to_string(), 1, 1)]);
    let mut a = ZipArchive::new(Cursor::new(&input[..])).expect("fixture opens");
    let mut s = String::new();
    a.by_name("a.txt").unwrap().read_to_string(&mut s).unwrap();
    assert_eq!(s, "hello");

    let out = append_one(input);
    for (name, sentinels, values) in central_headers(&out) {
        assert_eq!(
            values, sentinels,
            "{name}: ZIP64 field has {values} values but the header marks {sentinels}"
        );
    }
    let mut a = ZipArchive::new(Cursor::new(out)).expect("appended archive opens");
    assert_eq!(a.len(), 2);
    let mut s = String::new();
    a.by_name("a.txt").unwrap().read_to_string(&mut s).unwrap();
    assert_eq!(s, "hello");
}

/// An entry whose compressed size field alone holds the sentinel (the uncompressed size fits):
/// the ZIP64 field stores the two sizes together, so the rewritten header must mark both, not
/// write a two-value field under one sentinel (which a spec reader takes as
/// compressed = uncompressed size).
#[test]
fn one_size_sentinel_marks_both_sizes() {
    let mut zip = {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        w.start_file("a.txt", opts).unwrap();
        w.write_all(b"hello").unwrap();
        w.finish().unwrap().into_inner()
    };
    let cdh = zip
        .windows(4)
        .position(|s| s == b"PK\x01\x02")
        .expect("central directory header");
    // Compressed size field (+20) to the sentinel, no ZIP64 field: zip2 reads the size as
    // 0xFFFFFFFF, which needs ZIP64 when written back.
    zip[cdh + 20..cdh + 24].copy_from_slice(&u32::MAX.to_le_bytes());

    let out = append_one(zip);
    for (name, sentinels, values) in central_headers(&out) {
        assert_eq!(
            values, sentinels,
            "{name}: ZIP64 field has {values} values but the header marks {sentinels}"
        );
    }
}

/// `finish` writes the central directory, and when the archive it appended to left bytes past the
/// new footer (here: the old central directory and footer), rewrites the same central directory
/// at the end. Both writes of a header must be identical; in a debug build `finish` asserts that
/// the rewrite ends exactly where the data ends.
#[test]
fn appending_with_leftover_bytes_finishes_and_keeps_entries() {
    let mut zip = archive_with_sentinel_offset(64);
    // Bytes past the footer that the next append will not overwrite.
    zip.extend_from_slice(&[0u8; 4096]);
    let len_before = zip.len();
    let out = append_one(zip);
    assert_eq!(
        out.len(),
        len_before,
        "the rewritten footer must end where the data ends"
    );
    let mut a = ZipArchive::new(Cursor::new(out)).expect("appended archive opens");
    assert_eq!(a.len(), 2);
    let mut s = String::new();
    a.by_name("b.txt").unwrap().read_to_string(&mut s).unwrap();
    assert_eq!(s, "world");
}
