//! A central directory can claim a size that no longer fits in the address space once it is
//! added to the data's start offset: a ZIP64 extended information field is free to hold any
//! u64. Methods that compute `data_start + compressed_size` must reject such an archive as
//! invalid, because plain addition overflows, which panics on debug builds and silently wraps
//! on release builds.

use std::io::Cursor;

use zip::ZipArchive;
use zip::result::ZipError;

/// The smallest archive whose single stored entry claims a compressed size of u64::MAX through
/// a ZIP64 extended information extra field. The entry's flag word marks a data descriptor so
/// the same bytes exercise the descriptor path, and its five real data bytes sit where the
/// local header promises them, so only the size arithmetic is out of range.
fn zip64_huge_compressed_size() -> Vec<u8> {
    let data = b"hello";
    let name = b"a.tx";
    let mut buf = Vec::new();

    // Local file header: version 20, data-descriptor flag, stored, five bytes of data.
    buf.extend_from_slice(&0x04034b50u32.to_le_bytes());
    buf.extend_from_slice(&20u16.to_le_bytes());
    buf.extend_from_slice(&0x0008u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0x12345678u32.to_le_bytes());
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(name.len() as u16).to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(name);
    buf.extend_from_slice(data);

    // Central directory entry: 32-bit size fields hold the 0xFFFFFFFF sentinel and the real
    // values live in the ZIP64 extended information extra field, which claims an uncompressed
    // size of 5 and a compressed size of u64::MAX.
    let cd_start = buf.len() as u64;
    buf.extend_from_slice(&0x02014b50u32.to_le_bytes());
    buf.extend_from_slice(&20u16.to_le_bytes());
    buf.extend_from_slice(&20u16.to_le_bytes());
    buf.extend_from_slice(&0x0008u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0x12345678u32.to_le_bytes());
    buf.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());
    buf.extend_from_slice(&0xFFFFFFFFu32.to_le_bytes());
    buf.extend_from_slice(&(name.len() as u16).to_le_bytes());
    buf.extend_from_slice(&(4u16 + 16u16).to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(name);
    buf.extend_from_slice(&0x0001u16.to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes());
    buf.extend_from_slice(&5u64.to_le_bytes());
    buf.extend_from_slice(&u64::MAX.to_le_bytes());
    let cd_len = buf.len() as u64 - cd_start;

    // End of central directory.
    buf.extend_from_slice(&0x06054b50u32.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&(cd_len as u32).to_le_bytes());
    buf.extend_from_slice(&(cd_start as u32).to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf
}

#[test]
fn has_overlapping_files_rejects_overflowing_compressed_size() {
    let mut archive = ZipArchive::new(Cursor::new(zip64_huge_compressed_size())).unwrap();
    assert!(matches!(
        archive.has_overlapping_files(),
        Err(ZipError::InvalidArchive(_))
    ));
}

#[test]
fn data_descriptor_lookup_rejects_overflowing_compressed_size() {
    let mut archive = ZipArchive::new(Cursor::new(zip64_huge_compressed_size())).unwrap();
    assert!(matches!(
        archive.by_index_with_data_descriptor(0),
        Err(ZipError::InvalidArchive(_))
    ));
}
