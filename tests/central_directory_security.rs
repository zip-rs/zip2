//! Regression tests for GHSA-w2w4-j3wm-rrpp:
//! 1. Rejection of unsupported multi-disk archives (Zip32 and Zip64)
//! 2. Defense against integer underflow panic during EOCD64 candidate evaluation

use std::io::Cursor;
use zip::ZipArchive;

/// Verifies that `zip2` rejects a 32-bit archive declaring multiple disks
/// (e.g. disk_number != 0 or number_of_files_on_this_disk != number_of_files).
#[test]
fn test_multidisk_zip32_rejection() {
    let mut data = Vec::new();

    // Central directory file header
    let cd_offset = data.len() as u32;
    data.extend_from_slice(b"PK\x01\x02"); // CDFH signature
    data.extend_from_slice(&20u16.to_le_bytes()); // version made by
    data.extend_from_slice(&20u16.to_le_bytes()); // version needed
    data.extend_from_slice(&0u16.to_le_bytes()); // flags
    data.extend_from_slice(&0u16.to_le_bytes()); // compression
    data.extend_from_slice(&0u16.to_le_bytes()); // time
    data.extend_from_slice(&0u16.to_le_bytes()); // date
    data.extend_from_slice(&0u32.to_le_bytes()); // crc32
    data.extend_from_slice(&0u32.to_le_bytes()); // compressed size
    data.extend_from_slice(&0u32.to_le_bytes()); // uncompressed size
    data.extend_from_slice(&4u16.to_le_bytes()); // file name length ("test")
    data.extend_from_slice(&0u16.to_le_bytes()); // extra field length
    data.extend_from_slice(&0u16.to_le_bytes()); // comment length
    data.extend_from_slice(&1u16.to_le_bytes()); // disk number = 1 (MULTI-DISK!)
    data.extend_from_slice(&0u16.to_le_bytes()); // internal attr
    data.extend_from_slice(&0u32.to_le_bytes()); // external attr
    data.extend_from_slice(&0u32.to_le_bytes()); // local header offset
    data.extend_from_slice(b"test"); // file name
    let cd_size = (data.len() - cd_offset as usize) as u32;

    // 32-bit EOCD declaring disk_number = 1, and total_files = 10, files_on_disk = 1
    data.extend_from_slice(b"PK\x05\x06"); // EOCD signature
    data.extend_from_slice(&1u16.to_le_bytes()); // disk number = 1 (Multi-disk!)
    data.extend_from_slice(&1u16.to_le_bytes()); // disk with central directory = 1
    data.extend_from_slice(&1u16.to_le_bytes()); // files on this disk = 1
    data.extend_from_slice(&10u16.to_le_bytes()); // total files = 10 (Rest of files on other disks!)
    data.extend_from_slice(&cd_size.to_le_bytes()); // CD size
    data.extend_from_slice(&cd_offset.to_le_bytes()); // CD offset
    data.extend_from_slice(&0u16.to_le_bytes()); // comment length

    let reader = Cursor::new(data);
    let archive_res = ZipArchive::new(reader);

    match archive_res {
        Ok(archive) => {
            panic!(
                "SECURITY REGRESSION: ZipArchive accepted a multi-disk archive! len={}",
                archive.len()
            );
        }
        Err(e) => {
            let err_msg = e.to_string();
            assert!(
                err_msg.contains("Multi-disk ZIP files are not supported"),
                "Expected multi-disk rejection error, but got: {err_msg}"
            );
        }
    }
}

/// Verifies that `zip2` rejects a ZIP64 archive declaring multiple disks
/// (e.g. z64.disk_number != 0 or z64.number_of_files_on_this_disk != z64.number_of_files).
#[test]
fn test_multidisk_zip64_rejection() {
    let mut data = Vec::new();

    // CDFH
    let cd_offset = data.len() as u64;
    data.extend_from_slice(b"PK\x01\x02");
    data.extend_from_slice(&45u16.to_le_bytes());
    data.extend_from_slice(&45u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&4u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(b"file");
    let cd_size = data.len() as u64 - cd_offset;

    // ZIP64 EOCD: disk_number = 1 (Multi-disk!)
    let eocd64_offset = data.len() as u64;
    data.extend_from_slice(b"PK\x06\x06");
    data.extend_from_slice(&44u64.to_le_bytes()); // record size
    data.extend_from_slice(&45u16.to_le_bytes());
    data.extend_from_slice(&45u16.to_le_bytes());
    data.extend_from_slice(&1u32.to_le_bytes()); // disk_number = 1 (Multi-disk!)
    data.extend_from_slice(&0u32.to_le_bytes()); // disk with cd = 0
    data.extend_from_slice(&1u64.to_le_bytes()); // files on disk = 1
    data.extend_from_slice(&5u64.to_le_bytes()); // total files = 5 (Multi-disk!)
    data.extend_from_slice(&cd_size.to_le_bytes());
    data.extend_from_slice(&cd_offset.to_le_bytes());

    // ZIP64 Locator
    data.extend_from_slice(b"PK\x06\x07");
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&eocd64_offset.to_le_bytes());
    data.extend_from_slice(&1u32.to_le_bytes());

    // 32-bit EOCD
    data.extend_from_slice(b"PK\x05\x06");
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0xFFFFu16.to_le_bytes());
    data.extend_from_slice(&0xFFFFu16.to_le_bytes());
    data.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    data.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());

    let reader = Cursor::new(data);
    let archive_res = ZipArchive::new(reader);

    match archive_res {
        Ok(archive) => {
            panic!(
                "SECURITY REGRESSION: ZipArchive accepted a ZIP64 multi-disk archive! len={}",
                archive.len()
            );
        }
        Err(e) => {
            let err_msg = e.to_string();
            assert!(
                err_msg.contains("Multi-disk ZIP files are not supported"),
                "Expected multi-disk rejection error, but got: {err_msg}"
            );
        }
    }
}

/// Verifies that an attacker-manipulated EOCD64 offset does not trigger an
/// integer underflow panic during candidate evaluation.
#[test]
fn test_eocd64_underflow_defense() {
    let mut data = Vec::new();

    // CDFH
    let cd_offset = data.len() as u64;
    data.extend_from_slice(b"PK\x01\x02");
    data.extend_from_slice(&45u16.to_le_bytes());
    data.extend_from_slice(&45u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&4u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(b"test");
    let cd_size = data.len() as u64 - cd_offset;

    // ZIP64 EOCD
    let eocd64_offset = data.len() as u64;
    data.extend_from_slice(b"PK\x06\x06");
    data.extend_from_slice(&44u64.to_le_bytes());
    data.extend_from_slice(&45u16.to_le_bytes());
    data.extend_from_slice(&45u16.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&1u64.to_le_bytes());
    data.extend_from_slice(&1u64.to_le_bytes());
    data.extend_from_slice(&cd_size.to_le_bytes());
    data.extend_from_slice(&cd_offset.to_le_bytes());

    // ZIP64 Locator with an invalid offset that exceeds physical eocd64_offset
    let crafted_large_offset = eocd64_offset + 50000;
    data.extend_from_slice(b"PK\x06\x07");
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&crafted_large_offset.to_le_bytes()); // Exceeds eocd64_offset
    data.extend_from_slice(&1u32.to_le_bytes());

    // 32-bit EOCD
    data.extend_from_slice(b"PK\x05\x06");
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0xFFFFu16.to_le_bytes());
    data.extend_from_slice(&0xFFFFu16.to_le_bytes());
    data.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    data.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());

    let reader = Cursor::new(data);
    // Must not panic with integer underflow
    let _ = ZipArchive::new(reader);
}
