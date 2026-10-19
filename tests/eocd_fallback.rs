//! If the last EOCD's central directory ends where it starts but fails to parse,
//! `ZipArchive::new` must error instead of falling back to an earlier EOCD.

use std::io::{Cursor, Read};
use zip::ZipArchive;

/// An NTFS extra field this crate rejects and other readers skip.
const BAD_EXTRA_FIELD: &[u8] = b"\xff\xff\xff\xff\x00";

/// Bytes before the archive, as in a self-extractor; its offsets are then relative, not absolute.
const LEADING_GARBAGE: &[u8] = b"#!/bin/sh\n# self-extractor stub\nexit 0\n";

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Appends a one-entry stored archive with absolute offsets. Returns the CD offset.
fn append_archive(
    out: &mut Vec<u8>,
    name: &[u8],
    data: &[u8],
    extra: &[u8],
    comment: &[u8],
) -> u32 {
    append_archive_at(out, 0, false, name, data, extra, comment)
}

/// Like `append_archive`, with offsets relative to `base` and optional ZIP64 end records.
fn append_archive_at(
    out: &mut Vec<u8>,
    base: usize,
    zip64: bool,
    name: &[u8],
    data: &[u8],
    extra: &[u8],
    comment: &[u8],
) -> u32 {
    let local_header = (out.len() - base) as u32;
    let name_len = name.len() as u16;
    fn common(v: &mut Vec<u8>, data: &[u8], extra: &[u8], name_len: u16) {
        let crc = crc32(data);
        v.extend_from_slice(&[20, 0, 0, 0, 0, 0]); // version needed, flags, method = stored
        v.extend_from_slice(&[0, 0, 0x21, 0]); // time, date
        v.extend_from_slice(&crc.to_le_bytes());
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&name_len.to_le_bytes());
        v.extend_from_slice(&(extra.len() as u16).to_le_bytes());
    }
    out.extend_from_slice(b"PK\x03\x04");
    common(out, data, extra, name_len);
    out.extend_from_slice(name);
    out.extend_from_slice(extra);
    out.extend_from_slice(data);
    let cd = (out.len() - base) as u32;
    out.extend_from_slice(b"PK\x01\x02");
    out.extend_from_slice(&[20, 3]); // version made by
    common(out, data, extra, name_len);
    out.extend_from_slice(&[0; 6]); // comment length, disk, internal attributes
    out.extend_from_slice(&(0o100644u32 << 16).to_le_bytes());
    out.extend_from_slice(&local_header.to_le_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(extra);
    let cd_size = (out.len() - base) as u32 - cd;
    if zip64 {
        let eocd64 = (out.len() - base) as u64;
        out.extend_from_slice(b"PK\x06\x06");
        out.extend_from_slice(&44u64.to_le_bytes()); // record size
        out.extend_from_slice(&[45, 3, 45, 0]); // version made by, version needed
        out.extend_from_slice(&[0; 8]); // disk numbers
        out.extend_from_slice(&1u64.to_le_bytes());
        out.extend_from_slice(&1u64.to_le_bytes());
        out.extend_from_slice(&u64::from(cd_size).to_le_bytes());
        out.extend_from_slice(&u64::from(cd).to_le_bytes());
        out.extend_from_slice(b"PK\x06\x07");
        out.extend_from_slice(&0u32.to_le_bytes()); // disk with the ZIP64 EOCD
        out.extend_from_slice(&eocd64.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes()); // number of disks
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]);
        out.extend_from_slice(&[0xff; 8]);
    } else {
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0]);
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd.to_le_bytes());
    }
    out.extend_from_slice(&(comment.len() as u16).to_le_bytes());
    out.extend_from_slice(comment);
    cd
}

fn names(archive: &ZipArchive<Cursor<Vec<u8>>>) -> Vec<String> {
    archive
        .file_names()
        .map(|name| name.unwrap().into_owned())
        .collect()
}

/// A benign archive, then one with an NTFS extra field this crate rejects and others skip.
#[test]
fn invalid_last_central_directory_is_an_error() {
    let mut v = Vec::new();
    append_archive(&mut v, b"benign.txt", b"nothing to see", b"", b"");
    append_archive(&mut v, b"payload.sh", b"#!/bin/sh\n", BAD_EXTRA_FIELD, b"");
    let result = ZipArchive::new(Cursor::new(v));
    assert!(
        result.is_err(),
        "fell back to an earlier EOCD: {:?}",
        result.map(|a| names(&a))
    );
}

/// Same, with garbage after the last EOCD (which other readers accept).
#[test]
fn invalid_last_central_directory_is_an_error_with_trailing_garbage() {
    let mut v = Vec::new();
    append_archive(&mut v, b"benign.txt", b"nothing to see", b"", b"");
    append_archive(&mut v, b"payload.sh", b"#!/bin/sh\n", BAD_EXTRA_FIELD, b"");
    v.extend_from_slice(b"garbage");
    assert!(ZipArchive::new(Cursor::new(v)).is_err());
}

/// The last EOCD's CD offset points at the earlier archive, but its CD size at the later one.
#[test]
fn central_directory_ending_at_eocd_takes_precedence() {
    for garbage in [&b""[..], LEADING_GARBAGE] {
        let mut v = garbage.to_vec();
        v.extend(cd_offset_variant());
        let mut archive = ZipArchive::new(Cursor::new(v)).unwrap();
        assert_eq!(names(&archive), ["payload.sh"]);
        let mut contents = String::new();
        archive
            .by_index(0)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents, "#!/bin/sh\n");
    }
}

fn cd_offset_variant() -> Vec<u8> {
    let mut v = Vec::new();
    let benign_cd = append_archive(&mut v, b"benign.txt", &[b'.'; 200], b"", b"");
    let second_start = v.len() as u32;
    let payload_cd = append_archive(&mut v, b"payload.sh", b"#!/bin/sh\n", b"", b"");
    let shift = payload_cd - benign_cd;
    let eocd = v.len() - 22;
    v[eocd + 16..eocd + 20].copy_from_slice(&benign_cd.to_le_bytes());
    let cdfh = payload_cd as usize;
    v[cdfh + 42..cdfh + 46].copy_from_slice(&(second_start - shift).to_le_bytes());
    v
}

/// A stray EOCD signature in the comment still falls back to the real record.
#[test]
fn stray_eocd_in_comment_still_falls_back() {
    let mut fake = Vec::new();
    fake.extend_from_slice(b"PK\x05\x06");
    fake.extend_from_slice(&[0, 0, 0, 0, 5, 0, 5, 0]); // claims 5 entries
    fake.extend_from_slice(&0u32.to_le_bytes()); // CD size
    fake.extend_from_slice(&0u32.to_le_bytes()); // CD offset
    fake.extend_from_slice(&0u16.to_le_bytes()); // comment length

    for garbage in [&b""[..], LEADING_GARBAGE] {
        let mut v = garbage.to_vec();
        append_archive_at(&mut v, garbage.len(), false, b"a.txt", b"hello", b"", &fake);
        let archive = ZipArchive::new(Cursor::new(v)).unwrap();
        assert_eq!(names(&archive), ["a.txt"]);
        assert_eq!(archive.comment(), &fake[..]);
    }
}

/// Leading garbage, then two archives built separately, so every offset is relative.
fn concatenated_after_garbage(zip64: bool, payload_extra: &[u8]) -> Vec<u8> {
    let mut v = LEADING_GARBAGE.to_vec();
    let base = v.len();
    append_archive_at(
        &mut v,
        base,
        zip64,
        b"benign.txt",
        b"nothing to see",
        b"",
        b"",
    );
    let base = v.len();
    append_archive_at(
        &mut v,
        base,
        zip64,
        b"payload.sh",
        b"#!/bin/sh\n",
        payload_extra,
        b"",
    );
    v
}

/// Like other readers, list the last archive, not the earlier one.
#[test]
fn last_archive_wins_with_leading_garbage() {
    for zip64 in [false, true] {
        let v = concatenated_after_garbage(zip64, b"");
        let mut archive = ZipArchive::new(Cursor::new(v)).unwrap();
        assert_eq!(names(&archive), ["payload.sh"], "zip64: {zip64}");
        let mut contents = String::new();
        archive
            .by_index(0)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents, "#!/bin/sh\n");
    }
}

/// The decoy with leading garbage is still an error.
#[test]
fn invalid_last_central_directory_is_an_error_with_leading_garbage() {
    for zip64 in [false, true] {
        let result = ZipArchive::new(Cursor::new(concatenated_after_garbage(
            zip64,
            BAD_EXTRA_FIELD,
        )));
        assert!(
            result.is_err(),
            "zip64: {zip64}: fell back to an earlier EOCD: {:?}",
            result.map(|a| names(&a))
        );
    }
}
