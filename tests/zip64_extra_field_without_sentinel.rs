//! An entry whose sizes and offset all fit in 32 bits has nothing to say in a ZIP64 extended
//! information extra field, but writers exist that attach one regardless, and at least one
//! attaches a malformed one. Reading such a block over the entry's own perfectly good fields
//! turns an archive that every other tool accepts into an unreadable one, so a value from this
//! block may only replace a field holding the 0xFFFFFFFF sentinel that asked for it.

use std::io::{Cursor, Read, Seek, Write};

use zip::{
    ZipArchive, ZipWriter,
    write::{ExtendedFileOptions, FileOptions},
};

/// The first two entries of a real EPUB that this crate could not read, kept byte for byte: the
/// boilerplate `mimetype` and `META-INF/container.xml`, with a fresh end of central directory
/// record. Nothing of the book itself is in it. Every entry after `mimetype` in the original
/// carries the same malformed block as the one modelled below.
const REAL_WORLD_EPUB: &[u8] = include_bytes!("data/zip64_extra_field_without_sentinel.epub");

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

/// The archive the tests above model, as it was actually found: an EPUB whose every entry carries
/// a 32 byte ZIP64 block, no sentinel anywhere, and whose reads used to fail with
/// `InvalidArchive("Invalid local file header")`.
#[test]
fn the_epub_this_came_from_reads() {
    let mut archive = ZipArchive::new(Cursor::new(REAL_WORLD_EPUB)).expect("archive should open");
    assert_eq!(archive.len(), 2);
    check_entries_line_up(&mut archive);

    // `mimetype` is stored, so it reads whatever features are on.
    let mut mimetype = archive.by_name("mimetype").unwrap();
    let mut read_back = String::new();
    mimetype.read_to_string(&mut read_back).unwrap();
    assert_eq!(read_back, "application/epub+zip");
}

/// A value that was ignored on the way in must not be written back out on the way through.
///
/// `Zip64ExtendedInformation` is the same type on both sides, and a `None` field means "this entry
/// has nothing to record here" to the writer. Keeping a value no sentinel asked for would make the
/// two sides disagree, and `ZipWriter::new_append` would then re-emit it: the entry's own 32 bit
/// relative offset field says one thing while the ZIP64 block it carries says another.
#[test]
fn a_value_no_sentinel_asked_for_is_not_written_back_out() {
    let writer = ZipWriter::new_append(Cursor::new(REAL_WORLD_EPUB.to_vec()))
        .expect("archive should open for append");
    let appended = writer.finish().unwrap().into_inner();

    for (name, block) in central_directory_zip64_blocks(&appended) {
        assert!(
            block.len() <= 16,
            "{name}: {} byte ZIP64 block written, so it carries a relative header offset that no sentinel asked for",
            block.len()
        );
    }

    let mut archive = ZipArchive::new(Cursor::new(appended)).expect("output should open");
    assert_eq!(archive.len(), 2);
    check_entries_line_up(&mut archive);
}

/// Every entry's local header is where the central directory says it is, and still declares the
/// sizes the central directory does. This is the check the bug used to fail: a relative offset
/// taken from a block no sentinel asked for pointed into the middle of the compressed data, and
/// the seek there found no `PK\x03\x04`.
///
/// It goes through the raw reader so that it holds whatever compression features are enabled;
/// `META-INF/container.xml` is deflated, and this crate can be built without a deflate decoder.
fn check_entries_line_up<R: Read + Seek>(archive: &mut ZipArchive<R>) {
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i).expect("entry should be found");
        assert!(
            entry.size() > 0 && entry.compressed_size() > 0,
            "{}: empty entry, fixture is not what the test expects",
            entry.name().unwrap()
        );
    }

    let container = archive
        .by_index_raw(1)
        .expect("META-INF/container.xml should be found");
    assert_eq!(container.name().unwrap(), "META-INF/container.xml");
    assert_eq!(container.size(), 270, "the entry's own size stands");
    assert_eq!(
        container.compressed_size(),
        186,
        "the entry's own compressed size stands"
    );
}

/// The same fixture read all the way through, where there is a deflate decoder to do it with.
#[cfg(feature = "deflate-flate2")]
#[test]
fn the_epub_this_came_from_reads_its_contents() {
    let mut archive = ZipArchive::new(Cursor::new(REAL_WORLD_EPUB)).expect("archive should open");
    let mut container = archive.by_name("META-INF/container.xml").unwrap();
    let mut read_back = String::new();
    container.read_to_string(&mut read_back).unwrap();
    assert!(read_back.contains("OEBPS/content.opf"));
}

/// The payload of every ZIP64 extended information block in the central directory, by entry name.
fn central_directory_zip64_blocks(archive: &[u8]) -> Vec<(String, Vec<u8>)> {
    let eocd = archive
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .expect("end of central directory record");
    let entries = u16::from_le_bytes(archive[eocd + 10..eocd + 12].try_into().unwrap());
    let mut at = u32::from_le_bytes(archive[eocd + 16..eocd + 20].try_into().unwrap()) as usize;

    let mut blocks = Vec::new();
    for _ in 0..entries {
        assert_eq!(
            &archive[at..at + 4],
            b"PK\x01\x02",
            "central directory entry"
        );
        let field = |o: usize| u16::from_le_bytes(archive[at + o..at + o + 2].try_into().unwrap());
        let (name_len, extra_len, comment_len) =
            (field(28) as usize, field(30) as usize, field(32) as usize);
        let name = String::from_utf8_lossy(&archive[at + 46..at + 46 + name_len]).into_owned();
        let extra = &archive[at + 46 + name_len..at + 46 + name_len + extra_len];

        let mut i = 0;
        while i + 4 <= extra.len() {
            let id = u16::from_le_bytes(extra[i..i + 2].try_into().unwrap());
            let len = u16::from_le_bytes(extra[i + 2..i + 4].try_into().unwrap()) as usize;
            if id == ZIP64_ID {
                blocks.push((name.clone(), extra[i + 4..i + 4 + len].to_vec()));
            }
            i += 4 + len;
        }
        at += 46 + name_len + extra_len + comment_len;
    }
    blocks
}
