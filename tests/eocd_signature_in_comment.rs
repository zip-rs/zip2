//! An archive comment that contains an end-of-central-directory signature must not make readers
//! pick that fake record over the real one, whose comment ends exactly at the end of the file
//! (found by fuzz_write).

use std::io::{Cursor, Write};
use zip::ZipArchive;
use zip::write::{SimpleFileOptions, ZipWriter};

fn archive_with_fake_eocd_in_comment() -> (Vec<u8>, Vec<u8>) {
    // A complete empty EOCD record followed by a few bytes, so it doesn't end at EOF itself.
    let mut comment = b"junk".to_vec();
    comment.extend_from_slice(b"PK\x05\x06");
    comment.extend_from_slice(&[0; 18]);
    comment.extend_from_slice(b"tail");

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .set_raw_comment(comment.clone().into_boxed_slice())
        .unwrap();
    writer
        .start_file("a.txt", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"hello").unwrap();
    (writer.finish().unwrap().into_inner(), comment)
}

#[test]
fn fake_eocd_in_comment_is_ignored() {
    let (bytes, comment) = archive_with_fake_eocd_in_comment();
    let archive = ZipArchive::new(Cursor::new(bytes.clone())).unwrap();
    assert_eq!(archive.len(), 1);
    assert_eq!(archive.comment(), &comment[..]);

    let writer = ZipWriter::new_append(Cursor::new(bytes)).unwrap();
    assert_eq!(writer.get_raw_comment(), &comment[..]);
}

#[test]
fn fake_eocd_in_comment_is_ignored_with_trailing_bytes() {
    // E.g. the stale tail `ZipWriter::finish` leaves after an aborted file when it can't truncate.
    let (mut bytes, comment) = archive_with_fake_eocd_in_comment();
    bytes.extend_from_slice(&[0xFF; 7]);
    let archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    assert_eq!(archive.len(), 1);
    assert_eq!(archive.comment(), &comment[..]);
}
