//! An entry whose sizes and offset all fit in 32 bits has nothing to say in a ZIP64 extended
//! information extra field, but writers exist that attach one regardless, and at least one
//! attaches a malformed one. Reading such a block over the entry's own perfectly good fields
//! turns an archive that every other tool accepts into an unreadable one, so a value from this
//! block may only replace a field holding the 0xFFFFFFFF sentinel that asked for it.

use std::io::{Cursor, Read, Write};

use zip::{
    ZipArchive, ZipWriter,
    write::{ExtendedFileOptions, FileOptions},
};

/// Header id the archive is built with, swapped for the ZIP64 id once it is finished: the
/// writer rightly refuses to emit a custom field under 0x0001, and rewriting the two id bytes
/// afterwards leaves every length and offset exactly where the writer put them.
const PLACEHOLDER_ID: u16 = 0x7a7a;
const ZIP64_ID: u16 = 0x0001;

const CONTENT: &[u8] = b"the entry's own sizes and offset are the ones to trust";

/// The 32 byte payload of the malformed block, shaped like the one found in the wild. A correct
/// block is `[uncompressed u64][compressed u64][header offset u64][disk u32]`; this one begins
/// with eight bytes belonging to the NTFS block's header (a reserved u32, then tag 0x0001 and
/// size 0x0018), so every value sits eight bytes to the right of where a reader looks for it.
/// The values here are deliberately nothing like the entry's real ones: if any of them were
/// applied, the archive would not read.
fn malformed_payload() -> Vec<u8> {
    let mut payload = Vec::with_capacity(32);
    payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x18, 0x00]);
    payload.extend_from_slice(&u64::to_le_bytes(0x0018_0100_0000_0000));
    payload.extend_from_slice(&u64::to_le_bytes(270));
    payload.extend_from_slice(&u64::to_le_bytes(186));
    payload
}

/// Rewrites every `PLACEHOLDER_ID` field header to `ZIP64_ID`, in place and in both the local
/// and the central header, without moving a byte.
fn promote_placeholder_to_zip64(archive: &mut [u8], payload_len: u16) {
    let mut needle = Vec::with_capacity(4);
    needle.extend_from_slice(&PLACEHOLDER_ID.to_le_bytes());
    needle.extend_from_slice(&payload_len.to_le_bytes());
    let mut promoted = 0;
    for start in 0..archive.len().saturating_sub(needle.len()) {
        if &archive[start..start + needle.len()] == needle.as_slice() {
            archive[start..start + 2].copy_from_slice(&ZIP64_ID.to_le_bytes());
            promoted += 1;
        }
    }
    assert_eq!(
        promoted, 2,
        "expected the field in both the local and central header"
    );
}

fn archive_with_malformed_zip64_block() -> Vec<u8> {
    let payload = malformed_payload();
    let mut options: FileOptions<ExtendedFileOptions> = FileOptions::default();
    // Added to the local header, from where the writer mirrors it into the central directory,
    // so both headers carry it exactly as the archives found in the wild do.
    options
        .add_extra_field(PLACEHOLDER_ID, &payload, false)
        .expect("extra field should be accepted");

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("hello.txt", options).unwrap();
    writer.write_all(CONTENT).unwrap();
    let mut archive = writer.finish().unwrap().into_inner();

    let payload_len = u16::try_from(payload.len()).unwrap();
    promote_placeholder_to_zip64(&mut archive, payload_len);
    archive
}

#[test]
fn a_zip64_block_no_field_asked_for_does_not_override_the_entry() {
    let bytes = archive_with_malformed_zip64_block();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).expect("archive should open");
    assert_eq!(archive.len(), 1);

    let mut entry = archive.by_name("hello.txt").expect("entry should be found");
    assert_eq!(
        entry.size(),
        CONTENT.len() as u64,
        "the entry's own uncompressed size stands"
    );
    let mut read_back = Vec::new();
    entry.read_to_end(&mut read_back).unwrap();
    assert_eq!(read_back, CONTENT);
}

/// The same archive read by the streaming reader, which parses the local header rather than the
/// central directory and so reaches the block by a different path.
#[test]
fn the_streaming_reader_also_ignores_such_a_block() {
    let bytes = archive_with_malformed_zip64_block();
    let mut reader = Cursor::new(bytes);
    let mut entry = zip::read::read_zipfile_from_stream(&mut reader)
        .expect("local header should parse")
        .expect("there should be an entry");
    assert_eq!(entry.name().unwrap(), "hello.txt");
    let mut read_back = Vec::new();
    entry.read_to_end(&mut read_back).unwrap();
    assert_eq!(read_back, CONTENT);
}
