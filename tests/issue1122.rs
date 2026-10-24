#![cfg(feature = "_deflate-any")]
use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};
use zip::result::ZipResult;

fn local_zip64_sizes(bytes: &[u8]) -> (u64, u64) {
    let name_len = u16::from_le_bytes([bytes[26], bytes[27]]) as usize;
    let extra = 30 + name_len;
    assert_eq!(u16::from_le_bytes([bytes[extra], bytes[extra + 1]]), 0x0001);
    let u = u64::from_le_bytes(bytes[extra + 4..extra + 12].try_into().unwrap());
    let c = u64::from_le_bytes(bytes[extra + 12..extra + 20].try_into().unwrap());
    (u, c)
}

#[test]
fn test_issue1122() -> ZipResult<()> {
    let opts = SimpleFileOptions::default().large_file(true);

    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    w.add_directory("dir/", opts).unwrap();
    let dir = w.finish().unwrap().into_inner();

    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    w.start_file("empty.txt", opts).unwrap();
    w.write_all(b"").unwrap();
    let file = w.finish().unwrap().into_inner();

    // Directory entry has no content, but file entry has an empty DEFLATE block.
    for (label, bytes, expected_compressed_size) in [("directory", dir, 0), ("empty file", file, 2)] {
        let (u, c) = local_zip64_sizes(&bytes);
        println!("{label}: local ZIP64 extra uncompressed={u:#x} compressed={c:#x}");
        let mut a = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let f = a.by_index(0).unwrap();
        assert_eq!(0, f.size(), "{label}: uncompressed size");
        assert_eq!(expected_compressed_size, f.compressed_size(), "{label}: compressed size");
    }
    Ok(())
}