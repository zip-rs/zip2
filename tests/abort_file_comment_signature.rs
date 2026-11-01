//! Regression test for a fuzz_write failure in zip-rs/zip2 PR #1175: after `abort_file`, a few
//! stale bytes could be left (zeroed) after the archive comment. If the comment ended with an EOCD
//! signature, those zeros turned it into a valid-looking empty EOCD that readers picked instead
//! of the real one, so the archive reopened with an empty comment and no files.

use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

#[test]
fn abort_file_leaves_no_phantom_eocd_after_comment() {
    // 24-byte comment that ends with an EOCD signature.
    let comment: Vec<u8> = [&b"cPK\x05\x06"[..], &[b'9'; 15], &b"PK\x05\x06"[..]].concat();
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (keep, data_len) in [false, true]
        .into_iter()
        .flat_map(|k| (0..80).map(move |n| (k, n)))
    {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.set_raw_comment(comment.clone().into()).unwrap();
        if keep {
            writer.start_file("keep", options).unwrap();
            writer.write_all(b"kept").unwrap();
        }
        writer.start_file("a", options).unwrap();
        writer.write_all(&vec![0xAA; data_len]).unwrap();
        writer.abort_file().unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let archive = ZipArchive::new(Cursor::new(&bytes[..]))
            .unwrap_or_else(|e| panic!("data_len={data_len}: {e}"));
        assert_eq!(archive.comment(), &comment[..], "data_len={data_len}");
        assert_eq!(archive.len(), usize::from(keep), "data_len={data_len}");
        let writer = ZipWriter::new_append(Cursor::new(bytes)).unwrap();
        assert_eq!(
            writer.get_raw_comment(),
            &comment[..],
            "data_len={data_len}"
        );
    }
}

use zip::ZipUpdate;
