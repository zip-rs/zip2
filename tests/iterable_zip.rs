//! Tests for the index-free iterable ZIP reader.

use std::io::{Cursor, Read, Seek};

use zip::ZipArchive;
use zip::ZipReadOptions;
use zip::read::Config;
use zip::result::ZipError;
use zip::unstable::read::{ZipIterable, ZipIterableFiles};
#[cfg(feature = "deflate")]
use zip::write::SimpleFileOptions;

/// Builds a minimal ZIP archive with stored entries, including duplicate names if requested.
fn raw_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = Vec::new();
    let mut central_directory = Vec::new();

    for (name, data) in entries {
        let local_header_offset = archive.len() as u32;
        let crc = crc32fast::hash(data);
        let size = data.len() as u32;
        let name_len = name.len() as u16;

        // Local file header.
        archive.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        archive.extend_from_slice(&20u16.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(&crc.to_le_bytes());
        archive.extend_from_slice(&size.to_le_bytes());
        archive.extend_from_slice(&size.to_le_bytes());
        archive.extend_from_slice(&name_len.to_le_bytes());
        archive.extend_from_slice(&0u16.to_le_bytes());
        archive.extend_from_slice(name.as_bytes());
        archive.extend_from_slice(data);

        // Central directory header.
        central_directory.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central_directory.extend_from_slice(&20u16.to_le_bytes());
        central_directory.extend_from_slice(&20u16.to_le_bytes());
        central_directory.extend_from_slice(&0u16.to_le_bytes());
        central_directory.extend_from_slice(&0u16.to_le_bytes());
        central_directory.extend_from_slice(&0u16.to_le_bytes());
        central_directory.extend_from_slice(&0u16.to_le_bytes());
        central_directory.extend_from_slice(&crc.to_le_bytes());
        central_directory.extend_from_slice(&size.to_le_bytes());
        central_directory.extend_from_slice(&size.to_le_bytes());
        central_directory.extend_from_slice(&name_len.to_le_bytes());
        central_directory.extend_from_slice(&0u16.to_le_bytes());
        central_directory.extend_from_slice(&0u16.to_le_bytes());
        central_directory.extend_from_slice(&0u16.to_le_bytes());
        central_directory.extend_from_slice(&0u16.to_le_bytes());
        central_directory.extend_from_slice(&0u32.to_le_bytes());
        central_directory.extend_from_slice(&local_header_offset.to_le_bytes());
        central_directory.extend_from_slice(name.as_bytes());
    }

    let central_directory_offset = archive.len() as u32;
    let central_directory_size = central_directory.len() as u32;
    archive.extend_from_slice(&central_directory);

    archive.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    archive.extend_from_slice(&0u16.to_le_bytes());
    archive.extend_from_slice(&0u16.to_le_bytes());
    archive.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    archive.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    archive.extend_from_slice(&central_directory_size.to_le_bytes());
    archive.extend_from_slice(&central_directory_offset.to_le_bytes());
    archive.extend_from_slice(&0u16.to_le_bytes());

    archive
}

fn read_entry<R: Read + Seek>(
    entry: zip::read::ZipFileEntry<'_>,
    files: &mut ZipIterableFiles<R>,
) -> Vec<u8> {
    let mut data = Vec::new();
    entry
        .with_reader(files.reader_mut(), ZipReadOptions::new())
        .unwrap()
        .read_to_end(&mut data)
        .unwrap();
    data
}

#[test]
fn iterates_entries_in_central_directory_order() {
    let bytes = raw_zip(&[
        ("a.txt", b"alpha"),
        ("dir/b.bin", b"bravo"),
        ("c.txt", b"charlie"),
    ]);
    let mut archive = ZipIterable::try_new(Cursor::new(bytes), Config::default()).unwrap();

    assert_eq!(archive.len(), 3);
    assert!(!archive.is_empty());

    let files = archive.files().unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!(files.size_hint(), (3, Some(3)));

    let mut names = Vec::new();
    let mut contents = Vec::new();
    while let Some(entry) = files.next() {
        let entry = entry.unwrap();
        names.push(entry.name().unwrap().into_owned());
        contents.push(read_entry(entry, files));
    }

    assert_eq!(names, ["a.txt", "dir/b.bin", "c.txt"]);
    assert_eq!(contents, [b"alpha".as_slice(), b"bravo", b"charlie"]);
    assert!(files.is_empty());
    assert!(files.next().is_none());
}

#[test]
fn files_starts_a_new_pass() {
    let bytes = raw_zip(&[("one.txt", b"1"), ("two.txt", b"2")]);
    let mut archive = ZipIterable::try_new(Cursor::new(bytes), Config::default()).unwrap();

    assert_eq!(archive.files().unwrap().count(), 2);
    assert_eq!(archive.files().unwrap().count(), 2);
}

#[test]
fn duplicate_names_are_all_yielded() {
    let bytes = raw_zip(&[("a.txt", b"first"), ("a.txt", b"second")]);

    let mut archive = ZipIterable::try_new(Cursor::new(bytes.clone()), Config::default()).unwrap();
    let files = archive.files().unwrap();
    assert_eq!(files.len(), 2);
    let first = files.next().unwrap().unwrap();
    assert_eq!(read_entry(first, files), b"first");
    let second = files.next().unwrap().unwrap();
    assert_eq!(read_entry(second, files), b"second");
    assert!(files.next().is_none());

    // ZipArchive keeps one entry per name, which is why the iterable reader exists.
    let indexed = ZipArchive::new(Cursor::new(bytes)).unwrap();
    assert_eq!(indexed.len(), 1);
    assert!(indexed.has_duplicate_names());
}

#[test]
fn empty_archive_yields_nothing() {
    let bytes = raw_zip(&[]);
    let mut archive = ZipIterable::try_new(Cursor::new(bytes), Config::default()).unwrap();

    assert_eq!(archive.len(), 0);
    assert!(archive.is_empty());
    assert!(archive.files().unwrap().next().is_none());
}

#[test]
fn reading_an_entry_does_not_disturb_iteration() {
    let bytes = raw_zip(&[("a.txt", b"alpha"), ("b.txt", b"bravo")]);
    let mut archive = ZipIterable::try_new(Cursor::new(bytes), Config::default()).unwrap();
    let files = archive.files().unwrap();

    let first = files.next().unwrap().unwrap();
    let first_data = read_entry(first, files);
    let second = files.next().unwrap().unwrap();
    let second_data = read_entry(second, files);

    assert_eq!(first_data, b"alpha");
    assert_eq!(second_data, b"bravo");
    assert!(files.next().is_none());
}

#[test]
fn handles_prepended_data() {
    let mut prefixed = b"#!/bin/sh\nexit 0\n".to_vec();
    prefixed.extend_from_slice(&raw_zip(&[("a.txt", b"alpha")]));

    let mut archive = ZipIterable::try_new(Cursor::new(prefixed), Config::default()).unwrap();
    let files = archive.files().unwrap();
    let entry = files.next().unwrap().unwrap();
    assert_eq!(read_entry(entry, files), b"alpha");
}

#[test]
fn truncated_archive_is_an_error() {
    let mut bytes = raw_zip(&[("a.txt", b"alpha")]);
    bytes.truncate(bytes.len() - 10);
    assert!(ZipIterable::try_new(Cursor::new(bytes), Config::default()).is_err());
}

#[test]
fn broken_central_header_stops_the_pass() {
    let bytes = raw_zip(&[("a.txt", b"alpha"), ("b.txt", b"bravo")]);
    // The central directory offset is stored in the EOCD, six bytes before its two-byte comment
    // length at the end of the archive.
    let central_directory_offset =
        u32::from_le_bytes(bytes[bytes.len() - 6..bytes.len() - 2].try_into().unwrap()) as usize;
    let first_header_len = 46 + "a.txt".len();
    let mut bytes = bytes;
    let second_header = central_directory_offset + first_header_len;
    bytes[second_header..second_header + 4].copy_from_slice(&[0, 0, 0, 0]);

    let mut archive = ZipIterable::try_new(Cursor::new(bytes), Config::default()).unwrap();
    let files = archive.files().unwrap();

    let first = files.next().unwrap().unwrap();
    assert_eq!(first.name().unwrap(), "a.txt");
    assert!(files.next().unwrap().is_err());
    assert!(files.next().is_none());
}

#[test]
#[cfg(feature = "deflate")]
fn matches_zip_archive_for_deflated_archives() {
    use std::io::Write;
    use zip::CompressionMethod;

    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for index in 0..50 {
        writer
            .start_file(format!("entry-{index}.txt"), options)
            .unwrap();
        write!(writer, "contents of entry {index}").unwrap();
    }
    let bytes = writer.finish().unwrap().into_inner();

    let mut indexed = ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let expected: Vec<_> = (0..indexed.len())
        .map(|index| {
            let mut file = indexed.by_index(index).unwrap();
            let name = file.name().unwrap().into_owned();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            (name, data)
        })
        .collect();

    let mut iterable = ZipIterable::try_new(Cursor::new(&bytes), Config::default()).unwrap();
    let files = iterable.files().unwrap();
    let mut actual = Vec::new();
    while let Some(entry) = files.next() {
        let entry = entry.unwrap();
        let name = entry.name().unwrap().into_owned();
        actual.push((name.clone(), read_entry(entry, files)));
    }

    assert_eq!(actual, expected);
}

/// Overwrites the first central-directory header's file name in place.
fn spoof_first_central_name(bytes: &mut [u8], name: &[u8]) {
    let central = bytes
        .windows(4)
        .position(|s| s == b"PK\x01\x02")
        .expect("central directory header must exist");
    let name_len = u16::from_le_bytes(bytes[central + 28..central + 30].try_into().unwrap());
    assert_eq!(usize::from(name_len), name.len());
    bytes[central + 46..central + 46 + name.len()].copy_from_slice(name);
}

#[test]
fn enforce_header_consistency_rejects_dual_header_mismatch() {
    let mut bytes = raw_zip(&[("cron_worker.sh", b"#!/bin/sh\necho pwned\n")]);
    spoof_first_central_name(&mut bytes, b"benign_doc.pdf");

    // Without the option the central directory name wins, for backwards compatibility.
    let mut lenient = ZipIterable::try_new(Cursor::new(bytes.clone()), Config::default()).unwrap();
    let entry = lenient.files().unwrap().next().unwrap().unwrap();
    assert_eq!(entry.name().unwrap(), "benign_doc.pdf");

    // With the option, the mismatch is reported and stops the pass.
    let mut strict = ZipIterable::try_new(Cursor::new(bytes), Config::default())
        .unwrap()
        .enforce_header_consistency(true);
    let files = strict.files().unwrap();
    match files.next() {
        Some(Err(ZipError::InvalidArchive(message))) => {
            assert!(message.contains("filename mismatch"), "{message}");
        }
        other => panic!("expected a header inconsistency error, got {other:?}"),
    }
    assert!(files.next().is_none());
}

#[test]
fn enforce_header_consistency_rejects_local_crc_mismatch() {
    let mut bytes = raw_zip(&[("a.txt", b"alpha")]);
    // The CRC32 lives at offset 14 of the local file header.
    bytes[14..18].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());

    let mut archive = ZipIterable::try_new(Cursor::new(bytes), Config::default())
        .unwrap()
        .enforce_header_consistency(true);
    let files = archive.files().unwrap();
    match files.next() {
        Some(Err(ZipError::InvalidArchive(message))) => {
            assert!(message.contains("CRC32"), "{message}");
        }
        other => panic!("expected a header inconsistency error, got {other:?}"),
    }
}

#[test]
fn enforce_header_consistency_accepts_valid_archive() {
    let bytes = raw_zip(&[("a.txt", b"alpha"), ("dir/b.bin", b"bravo")]);
    let mut archive = ZipIterable::try_new(Cursor::new(bytes), Config::default())
        .unwrap()
        .enforce_header_consistency(true);
    let files = archive.files().unwrap();

    let mut names = Vec::new();
    let mut contents = Vec::new();
    while let Some(entry) = files.next() {
        let entry = entry.unwrap();
        names.push(entry.name().unwrap().into_owned());
        contents.push(read_entry(entry, files));
    }

    assert_eq!(names, ["a.txt", "dir/b.bin"]);
    assert_eq!(contents, [b"alpha".as_slice(), b"bravo"]);
}

#[test]
fn enforce_header_consistency_accepts_prepended_data() {
    let mut prefixed = b"#!/bin/sh\nexit 0\n".to_vec();
    prefixed.extend_from_slice(&raw_zip(&[("a.txt", b"alpha")]));

    let mut archive = ZipIterable::try_new(Cursor::new(prefixed), Config::default())
        .unwrap()
        .enforce_header_consistency(true);
    let files = archive.files().unwrap();
    let entry = files.next().unwrap().unwrap();
    assert_eq!(read_entry(entry, files), b"alpha");
}

#[test]
fn enforce_header_consistency_still_yields_duplicate_names() {
    // Duplicate names are the reason this reader exists, so they are not an inconsistency.
    let bytes = raw_zip(&[("a.txt", b"first"), ("a.txt", b"second")]);
    let mut archive = ZipIterable::try_new(Cursor::new(bytes), Config::default())
        .unwrap()
        .enforce_header_consistency(true);
    let files = archive.files().unwrap();

    let first = files.next().unwrap().unwrap();
    assert_eq!(read_entry(first, files), b"first");
    let second = files.next().unwrap().unwrap();
    assert_eq!(read_entry(second, files), b"second");
    assert!(files.next().is_none());
}
