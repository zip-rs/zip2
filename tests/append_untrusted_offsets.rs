//! Regression tests: `ZipWriter::new_append` on archives whose central directory points past EOF
//! must not make later operations seek/allocate ~4 GiB or panic.
use std::io::{Cursor, Write};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn archive_with_one_file() -> (Vec<u8>, usize) {
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    w.start_file(
        "a",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )
    .unwrap();
    w.write_all(b"hello").unwrap();
    let v = w.finish().unwrap().into_inner();
    let cd = v.windows(4).position(|x| x == b"PK\x01\x02").unwrap();
    (v, cd)
}

#[test]
fn abort_file_after_append_with_header_offset_past_eof() {
    let (mut v, cd) = archive_with_one_file();
    // Central directory: relative offset of local header := 0xFFFF_FFF0
    v[cd + 42..cd + 46].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    let Ok(mut w) = ZipWriter::new_append(Cursor::new(v)) else {
        return;
    };
    let _ = w.abort_file();
    w.set_raw_comment(Box::new([1, 2]))
        .expect("Error from set_raw_comment");
    if let Ok(out) = w.finish() {
        assert!(
            out.get_ref().len() < 1 << 20,
            "archive grew to {} bytes",
            out.get_ref().len()
        );
    }
}

#[test]
fn deep_copy_after_append_with_data_start_past_eof() {
    let (mut v, cd) = archive_with_one_file();
    // Local header extra-field length := 0xFFFF, so data_start lies past EOF;
    // central directory compressed size := 0xFFFF_FFF0.
    v[28..30].copy_from_slice(&0xFFFFu16.to_le_bytes());
    v[cd + 20..cd + 24].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    let Ok(mut w) = ZipWriter::new_append(Cursor::new(v)) else {
        return;
    };
    assert!(w.deep_copy_file("a", "b").is_err());
}

use zip::ZipUpdate;
