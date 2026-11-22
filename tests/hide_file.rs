//! Test about removing file

use std::io::{Cursor, Read, Write};

use zip::CompressionMethod;
use zip::ZipArchive;
use zip::ZipWriter;
use zip::result::{ZipError, ZipResult};
use zip::write::{FullFileOptions, SimpleFileOptions};

#[test]
fn hide_file_drops_the_entry_but_keeps_the_others() -> ZipResult<()> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for name in ["a.txt", "b.txt", "c.txt"] {
        zip.start_file(
            name,
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )?;
        zip.write_all(name.as_bytes())?;
    }
    zip.hide_file("b.txt")?;

    let mut archive = ZipArchive::new(zip.finish()?)?;
    assert_eq!(archive.len(), 2);
    assert!(archive.by_name("b.txt").is_err());
    // The survivors still read back correctly: removing an entry must not
    // disturb data written after it.
    for name in ["a.txt", "c.txt"] {
        let mut contents = String::new();
        std::io::Read::read_to_string(&mut archive.by_name(name)?, &mut contents)?;
        assert_eq!(contents, name);
    }
    Ok(())
}

/// Central-directory order is `files` order, so a removal must not
/// reshuffle the entries around it.
#[test]
fn hide_file_preserves_the_order_of_surviving_entries() -> ZipResult<()> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        zip.start_file(
            name,
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )?;
        zip.write_all(b"x")?;
    }
    zip.hide_file("b.txt")?;

    let archive = ZipArchive::new(zip.finish()?)?;
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.name_for_index(i).unwrap().unwrap().to_string())
        .collect();
    assert_eq!(names, ["a.txt", "c.txt", "d.txt"]);
    Ok(())
}

/// The point of the method: the removed entry's bytes stay put, so the
/// cost of a removal does not scale with what follows it.
#[test]
fn hide_file_does_not_rewrite_the_archive() -> ZipResult<()> {
    let big = vec![b'x'; 64 * 1024];
    // Stored, not deflated: the whole point is to observe the removed
    // entry's bytes still occupying the file, and 64 KiB of one byte
    // deflates to almost nothing.
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let mut with_all = ZipWriter::new(Cursor::new(Vec::new()));
    with_all.start_file("gone.txt", stored)?;
    with_all.write_all(&big)?;
    with_all.start_file("kept.txt", stored)?;
    with_all.write_all(b"kept")?;
    let removed_len = {
        let mut zip = with_all;
        zip.hide_file("gone.txt")?;
        zip.finish()?.into_inner().len()
    };

    let mut only_kept = ZipWriter::new(Cursor::new(Vec::new()));
    only_kept.start_file("kept.txt", stored)?;
    only_kept.write_all(b"kept")?;
    let fresh_len = only_kept.finish()?.into_inner().len();

    assert!(
        removed_len > fresh_len + big.len() / 2,
        "removed entry's bytes should still be present ({removed_len} vs {fresh_len})"
    );
    Ok(())
}

/// A name freed by removal can be reused, which is what makes
/// replace-an-entry possible without rewriting.
#[test]
fn hide_file_frees_the_name_for_reuse() -> ZipResult<()> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(
        "a.txt",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    zip.write_all(b"first")?;
    zip.hide_file("a.txt")?;
    zip.start_file(
        "a.txt",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    zip.write_all(b"second")?;

    let mut archive = ZipArchive::new(zip.finish()?)?;
    assert_eq!(archive.len(), 1);
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("a.txt")?, &mut contents)?;
    assert_eq!(contents, "second");
    Ok(())
}

/// Removing the entry currently being written finishes it first, so the
/// archive is left consistent rather than mid-entry.
#[test]
fn hide_file_can_remove_the_entry_being_written() -> ZipResult<()> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(
        "a.txt",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    zip.write_all(b"a")?;
    zip.start_file(
        "b.txt",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    zip.write_all(b"b")?;
    zip.hide_file("b.txt")?;

    let mut archive = ZipArchive::new(zip.finish()?)?;
    assert_eq!(archive.len(), 1);
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("a.txt")?, &mut contents)?;
    assert_eq!(contents, "a");
    Ok(())
}

#[test]
fn hide_file_reports_a_missing_entry() {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    assert!(matches!(
        zip.hide_file("absent.txt"),
        Err(ZipError::FileNotFound)
    ));
}

/// An entry sharing data with the removed one keeps working: removal
/// touches the directory, never the bytes.
#[test]
fn hide_file_leaves_a_shallow_copy_readable() -> ZipResult<()> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(
        "original.txt",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    zip.write_all(b"shared")?;
    zip.shallow_copy_file("original.txt", "copy.txt")?;
    zip.hide_file("original.txt")?;

    let mut archive = ZipArchive::new(zip.finish()?)?;
    assert_eq!(archive.len(), 1);
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("copy.txt")?, &mut contents)?;
    assert_eq!(contents, "shared");
    Ok(())
}

#[test]
fn hide_file_same_bytes() {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for idx in 0..=10 {
        let filename = format!("file_{idx}.txt");
        zip.start_file(
            filename,
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(b"shared").unwrap();
    }

    // we have a basic zip file
    let zip_raw = zip.finish_into_readable().unwrap().into_inner();
    let zip_raw_copy = zip_raw.clone().into_inner();

    let (central_dir_start, zip_modified) = {
        let zip = ZipArchive::new(zip_raw).unwrap();
        let central_dir_start = zip.central_directory_start() as usize;

        let mut zip = ZipWriter::new_append(zip.into_inner()).unwrap();
        zip.hide_file("file_5.txt").unwrap();

        let readable = zip
            .finish_into_readable()
            .unwrap()
            .into_inner()
            .into_inner();
        (central_dir_start, readable)
    };
    assert_eq!(central_dir_start, 507);
    // All local headers and entry payloads before the central directory stay untouched,
    // and rewriting the shorter central directory does not grow the archive.
    assert_eq!(
        zip_modified[0..central_dir_start],
        zip_raw_copy[0..central_dir_start]
    );
    assert_eq!(zip_modified.len(), zip_raw_copy.len());
}

#[test]
fn erase_file_overwrites_payload_with_zeroes_and_keeps_survivors() -> ZipResult<()> {
    let payload = vec![0xa5; 64 * 1024 + 37];
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("gone.txt", stored)?;
    zip.write_all(&payload)?;
    zip.start_file("kept.txt", stored)?;
    zip.write_all(b"keep me")?;

    let mut readable = zip.finish_into_readable()?;
    let (header_start, data_start, compressed_size) = {
        let gone = readable.by_name("gone.txt")?;
        (
            gone.header_start(),
            gone.data_start().unwrap(),
            gone.compressed_size(),
        )
    };
    let baseline = readable.into_inner().into_inner();

    let mut zip = ZipWriter::new_append(Cursor::new(baseline.clone()))?;
    zip.erase_file("gone.txt", false)?;
    let erased = zip.finish()?.into_inner();

    assert!(
        erased[header_start as usize..(data_start + compressed_size) as usize]
            .iter()
            .all(|byte| *byte == 0)
    );

    let mut archive = ZipArchive::new(Cursor::new(erased))?;
    assert_eq!(archive.len(), 1);
    assert!(archive.by_name("gone.txt").is_err());
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("kept.txt")?, &mut contents)?;
    assert_eq!(contents, "keep me");
    Ok(())
}

#[test]
fn erase_file_can_randomize_payload() -> ZipResult<()> {
    let payload = vec![0x3c; 64 * 1024 + 19];
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("gone.txt", stored)?;
    zip.write_all(&payload)?;
    let mut readable = zip.finish_into_readable()?;
    let (header_start, data_start, compressed_size) = {
        let gone = readable.by_name("gone.txt")?;
        (
            gone.header_start(),
            gone.data_start().unwrap(),
            gone.compressed_size(),
        )
    };
    let baseline = readable.into_inner().into_inner();

    let mut zip = ZipWriter::new_append(Cursor::new(baseline.clone()))?;
    zip.erase_file("gone.txt", true)?;
    let erased = zip.finish()?.into_inner();

    // Both the local header and the payload must be overwritten with non-zero random bytes.
    assert_ne!(
        &erased[header_start as usize..data_start as usize],
        &baseline[header_start as usize..data_start as usize]
    );
    assert!(
        erased[header_start as usize..data_start as usize]
            .iter()
            .any(|byte| *byte != 0)
    );
    assert_ne!(
        &erased[data_start as usize..(data_start + compressed_size) as usize],
        &payload[..]
    );
    assert!(
        erased[data_start as usize..(data_start + compressed_size) as usize]
            .iter()
            .any(|byte| *byte != 0)
    );
    let archive = ZipArchive::new(Cursor::new(erased))?;
    assert_eq!(archive.len(), 0);
    Ok(())
}

#[test]
fn erase_file_can_remove_the_entry_being_written() -> ZipResult<()> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(
        "a.txt",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )?;
    zip.write_all(b"first")?;
    zip.erase_file("a.txt", false)?;

    let archive = ZipArchive::new(zip.finish()?)?;
    assert_eq!(archive.len(), 0);
    Ok(())
}

#[test]
fn erase_file_reports_a_missing_entry() {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    assert!(matches!(
        zip.erase_file("absent.txt", false),
        Err(ZipError::FileNotFound)
    ));
}

#[test]
fn erase_file_overwrites_data_descriptor_and_rewinds_last_entry() -> ZipResult<()> {
    // ZIP32 (16-byte) and ZIP64 (24-byte) data descriptors with signature via new_stream.
    for (large_file, dd_size) in [(false, 16usize), (true, 24usize)] {
        let opts = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .large_file(large_file);

        // Middle entry with data descriptor followed by a survivor.
        let mut stream_buf = Vec::new();
        {
            let mut zip = ZipWriter::new_stream(&mut stream_buf);
            zip.start_file("gone.txt", opts)?;
            zip.write_all(b"secret payload")?;
            zip.start_file("kept.txt", opts)?;
            zip.write_all(b"keep me")?;
            zip.finish()?;
        }
        let mut readable = ZipArchive::new(Cursor::new(stream_buf))?;
        let (header_start, data_start, compressed_size) = {
            let gone = readable.by_name("gone.txt")?;
            (
                gone.header_start(),
                gone.data_start().unwrap(),
                gone.compressed_size(),
            )
        };
        let baseline = readable.into_inner().into_inner();

        let mut zip = ZipWriter::new_append(Cursor::new(baseline))?;
        zip.erase_file("gone.txt", false)?;
        let erased = zip.finish()?.into_inner();

        let desc_end = data_start as usize + compressed_size as usize + dd_size;
        assert!(
            erased[header_start as usize..desc_end]
                .iter()
                .all(|byte| *byte == 0),
            "local header, data, and {dd_size}-byte data descriptor should all be zeroed"
        );
        let mut archive = ZipArchive::new(Cursor::new(erased))?;
        assert_eq!(archive.len(), 1);
        let mut contents = String::new();
        archive.by_name("kept.txt")?.read_to_string(&mut contents)?;
        assert_eq!(contents, "keep me");

        // Last entry with data descriptor: erasing rewinds write position to header_start.
        let mut stream_buf = Vec::new();
        {
            let mut zip = ZipWriter::new_stream(&mut stream_buf);
            zip.start_file("kept.txt", opts)?;
            zip.write_all(b"keep me")?;
            zip.start_file("gone.txt", opts)?;
            zip.write_all(b"secret tail")?;
            zip.finish()?;
        }
        let mut readable = ZipArchive::new(Cursor::new(stream_buf))?;
        let (header_start, data_start, compressed_size) = {
            let gone = readable.by_name("gone.txt")?;
            (
                gone.header_start(),
                gone.data_start().unwrap(),
                gone.compressed_size(),
            )
        };
        let baseline = readable.into_inner().into_inner();

        let mut zip = ZipWriter::new_append(Cursor::new(baseline))?;
        zip.erase_file("gone.txt", false)?;
        zip.start_file("new.txt", opts)?;
        zip.write_all(b"reused")?;
        let final_buf = zip.finish()?.into_inner();

        let mut archive = ZipArchive::new(Cursor::new(final_buf))?;
        assert_eq!(archive.len(), 2);
        let new_header_start = archive.by_name("new.txt")?.header_start();
        assert_eq!(
            new_header_start, header_start,
            "new entry should reuse the erased last entry's header_start"
        );
        assert!(data_start + compressed_size + dd_size as u64 > header_start);
        let mut contents = String::new();
        archive.by_name("kept.txt")?.read_to_string(&mut contents)?;
        assert_eq!(contents, "keep me");
        contents.clear();
        archive.by_name("new.txt")?.read_to_string(&mut contents)?;
        assert_eq!(contents, "reused");
    }
    Ok(())
}

fn ieee_crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Builds a minimal valid Stored ZIP archive where each entry can have a 16-byte data
/// descriptor with signature, a 12-byte data descriptor without signature (APPNOTE 4.3.9.3),
/// or no descriptor.
fn build_custom_dd_zip(entries: &[(&str, &[u8], Option<bool>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cd = Vec::new();

    for &(name, data, dd_kind) in entries {
        let crc = ieee_crc32(data);
        let header_start = out.len() as u32;
        let name_bytes = name.as_bytes();
        let flags: u16 = if dd_kind.is_some() { 1 << 3 } else { 0 };
        let (lh_crc, lh_size) = if dd_kind.is_some() {
            (0u32, 0u32)
        } else {
            (crc, data.len() as u32)
        };

        // Local file header
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // Stored
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&lh_crc.to_le_bytes());
        out.extend_from_slice(&lh_size.to_le_bytes());
        out.extend_from_slice(&lh_size.to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(data);

        match dd_kind {
            Some(true) => {
                // 16-byte data descriptor with signature
                out.extend_from_slice(&0x0807_4b50u32.to_le_bytes());
                out.extend_from_slice(&crc.to_le_bytes());
                out.extend_from_slice(&(data.len() as u32).to_le_bytes());
                out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            }
            Some(false) => {
                // 12-byte data descriptor without signature
                out.extend_from_slice(&crc.to_le_bytes());
                out.extend_from_slice(&(data.len() as u32).to_le_bytes());
                out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            }
            None => {}
        }

        // Central directory header
        cd.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        cd.extend_from_slice(&20u16.to_le_bytes());
        cd.extend_from_slice(&20u16.to_le_bytes());
        cd.extend_from_slice(&flags.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&crc.to_le_bytes());
        cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u32.to_le_bytes());
        cd.extend_from_slice(&header_start.to_le_bytes());
        cd.extend_from_slice(name_bytes);
    }

    let cd_start = out.len() as u32;
    out.extend_from_slice(&cd);
    let cd_size = cd.len() as u32;

    // End of central directory
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[test]
fn erase_file_handles_dd_without_signature_and_crc_collision() -> ZipResult<()> {
    // 4-byte payload whose IEEE CRC-32 equals 0x08074b50 (the optional DD signature).
    const COLLIDING_PAYLOAD: &[u8] = &[0xac, 0x0a, 0x7a, 0xd5];
    const COLLIDING_CRC: u32 = 0x0807_4b50;
    const KEPT_PAYLOAD: &[u8] = b"keep me";
    assert_eq!(ieee_crc32(COLLIDING_PAYLOAD), COLLIDING_CRC);

    // 1. 12-byte DD without signature as the last entry (and with crc32 == 0x08074b50).
    let last_without_sig = build_custom_dd_zip(&[
        ("kept.txt", KEPT_PAYLOAD, None),
        ("gone.txt", COLLIDING_PAYLOAD, Some(false)),
    ]);
    let mut readable = ZipArchive::new(Cursor::new(last_without_sig))?;
    assert_eq!(readable.by_name("gone.txt")?.crc32(), COLLIDING_CRC);
    let (header_start, data_start, compressed_size) = {
        let gone = readable.by_name("gone.txt")?;
        (
            gone.header_start(),
            gone.data_start().unwrap(),
            gone.compressed_size(),
        )
    };
    let baseline = readable.into_inner().into_inner();
    let mut zip = ZipWriter::new_append(Cursor::new(baseline))?;
    zip.erase_file("gone.txt", false)?;
    let erased = zip.finish()?.into_inner();
    assert!(
        erased[header_start as usize..(data_start + compressed_size + 12) as usize]
            .iter()
            .all(|byte| *byte == 0)
    );
    let mut archive = ZipArchive::new(Cursor::new(erased))?;
    assert_eq!(archive.len(), 1);
    let mut contents = String::new();
    archive.by_name("kept.txt")?.read_to_string(&mut contents)?;
    assert_eq!(contents, "keep me");

    // 2. 12-byte DD without signature in the middle with crc32 == 0x08074b50 followed by kept.txt:
    // must wipe exact 12 bytes without clobbering kept.txt's local header signature.
    let mid_without_sig = build_custom_dd_zip(&[
        ("gone.txt", COLLIDING_PAYLOAD, Some(false)),
        ("kept.txt", KEPT_PAYLOAD, None),
    ]);
    let mut readable = ZipArchive::new(Cursor::new(mid_without_sig))?;
    let (header_start, data_start, compressed_size) = {
        let gone = readable.by_name("gone.txt")?;
        (
            gone.header_start(),
            gone.data_start().unwrap(),
            gone.compressed_size(),
        )
    };
    let baseline = readable.into_inner().into_inner();
    let mut zip = ZipWriter::new_append(Cursor::new(baseline))?;
    zip.erase_file("gone.txt", false)?;
    let erased = zip.finish()?.into_inner();
    assert!(
        erased[header_start as usize..(data_start + compressed_size + 12) as usize]
            .iter()
            .all(|byte| *byte == 0)
    );
    let mut archive = ZipArchive::new(Cursor::new(erased))?;
    assert_eq!(archive.len(), 1);
    contents.clear();
    archive.by_name("kept.txt")?.read_to_string(&mut contents)?;
    assert_eq!(contents, "keep me");

    // 3. 16-byte DD with signature and crc32 == 0x08074b50: must wipe all 16 bytes.
    let mid_with_sig = build_custom_dd_zip(&[
        ("gone.txt", COLLIDING_PAYLOAD, Some(true)),
        ("kept.txt", KEPT_PAYLOAD, None),
    ]);
    let mut readable = ZipArchive::new(Cursor::new(mid_with_sig))?;
    let (header_start, data_start, compressed_size) = {
        let gone = readable.by_name("gone.txt")?;
        (
            gone.header_start(),
            gone.data_start().unwrap(),
            gone.compressed_size(),
        )
    };
    let baseline = readable.into_inner().into_inner();
    let mut zip = ZipWriter::new_append(Cursor::new(baseline))?;
    zip.erase_file("gone.txt", false)?;
    let erased = zip.finish()?.into_inner();
    assert!(
        erased[header_start as usize..(data_start + compressed_size + 16) as usize]
            .iter()
            .all(|byte| *byte == 0)
    );
    let mut archive = ZipArchive::new(Cursor::new(erased))?;
    assert_eq!(archive.len(), 1);
    contents.clear();
    archive.by_name("kept.txt")?.read_to_string(&mut contents)?;
    assert_eq!(contents, "keep me");
    Ok(())
}

#[test]
fn erase_file_handles_dd_flag_without_descriptor_bytes() -> ZipResult<()> {
    // 0-byte directory with DD flag set in header but no descriptor bytes on disk (cf. #971).
    let mut buf = build_custom_dd_zip(&[("dir/", b"", None), ("kept.txt", b"keep me", None)]);
    // Set the data-descriptor bit (bit 3) on dir/'s local header and central directory entry.
    buf[6] |= 1 << 3;
    let cd_pos = buf
        .windows(4)
        .position(|w| w == 0x0201_4b50u32.to_le_bytes())
        .unwrap();
    buf[cd_pos + 8] |= 1 << 3;

    let mut zip = ZipWriter::new_append(Cursor::new(buf))?;
    zip.erase_file("dir/", false)?;
    let erased = zip.finish()?.into_inner();

    let mut archive = ZipArchive::new(Cursor::new(erased))?;
    assert_eq!(archive.len(), 1);
    let mut contents = String::new();
    archive.by_name("kept.txt")?.read_to_string(&mut contents)?;
    assert_eq!(contents, "keep me");
    Ok(())
}

#[test]
fn erase_file_rejects_shallow_copy_overlap_and_preserves_archive() -> ZipResult<()> {
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("a.txt", stored)?;
    zip.write_all(b"shared secret")?;
    zip.shallow_copy_file("a.txt", "b.txt")?;
    zip.start_file("c.txt", stored)?;
    zip.write_all(b"other file")?;

    // Must refuse to overwrite bytes shared with shallow copy "b.txt".
    assert!(zip.erase_file("a.txt", false).is_err());

    // Archive and all three entries must still be intact.
    let buf = zip.finish()?.into_inner();
    let mut archive = ZipArchive::new(Cursor::new(buf.clone()))?;
    assert_eq!(archive.len(), 3);
    for (name, expected) in [
        ("a.txt", "shared secret"),
        ("b.txt", "shared secret"),
        ("c.txt", "other file"),
    ] {
        let mut s = String::new();
        archive.by_name(name)?.read_to_string(&mut s)?;
        assert_eq!(s, expected);
    }

    // Once the alias is hidden, erasing "a.txt" succeeds cleanly.
    let mut zip = ZipWriter::new_append(Cursor::new(buf))?;
    zip.hide_file("b.txt")?;
    zip.erase_file("a.txt", false)?;
    let mut archive = ZipArchive::new(zip.finish()?)?;
    assert_eq!(archive.len(), 1);
    let mut s = String::new();
    archive.by_name("c.txt")?.read_to_string(&mut s)?;
    assert_eq!(s, "other file");
    Ok(())
}

#[test]
fn erase_file_error_restores_writer_position_and_preserves_survivors() -> ZipResult<()> {
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("first.txt", stored)?;
    zip.write_all(b"first survivor")?;
    zip.start_file("corrupt.txt", stored)?;
    zip.write_all(b"corrupt entry")?;
    zip.start_file("last.txt", stored)?;
    zip.write_all(b"last survivor")?;

    let mut readable = zip.finish_into_readable()?;
    let corrupt_header = readable.by_name("corrupt.txt")?.header_start() as usize;
    let mut buf = readable.into_inner().into_inner();
    // Corrupt the local file header signature of "corrupt.txt" so `data_start()` fails.
    buf[corrupt_header..corrupt_header + 4].copy_from_slice(&[0, 0, 0, 0]);

    let mut zip = ZipWriter::new_append(Cursor::new(buf))?;
    assert!(zip.erase_file("corrupt.txt", false).is_err());

    // Finishing after the failed erase_file must not overwrite "last.txt" with the central dir.
    let mut archive = ZipArchive::new(zip.finish()?)?;
    assert_eq!(archive.len(), 3);
    for (name, expected) in [
        ("first.txt", "first survivor"),
        ("last.txt", "last survivor"),
    ] {
        let mut s = String::new();
        archive.by_name(name)?.read_to_string(&mut s)?;
        assert_eq!(s, expected);
    }
    Ok(())
}

#[test]
fn erase_file_wipes_stale_central_directory_on_append_finish() -> ZipResult<()> {
    let secret_name = "top_secret_password_list_2026.txt";
    let secret_comment = "TOP_SECRET_FILE_COMMENT";
    for (with_survivor, randomize) in [(false, false), (false, true), (true, false), (true, true)] {
        let opts = FullFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .with_file_comment(secret_comment);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(secret_name, opts)?;
        zip.write_all(b"hunter2")?;
        if with_survivor {
            zip.start_file("kept.txt", stored)?;
            zip.write_all(b"still here")?;
        }
        let baseline = zip.finish()?.into_inner();

        let mut zip = ZipWriter::new_append(Cursor::new(baseline))?;
        zip.erase_file(secret_name, randomize)?;
        let erased = zip.finish()?.into_inner();

        assert!(
            !erased
                .windows(secret_name.len())
                .any(|w| w == secret_name.as_bytes()),
            "erased filename survived in output (with_survivor={with_survivor}, randomize={randomize})"
        );
        assert!(
            !erased
                .windows(secret_comment.len())
                .any(|w| w == secret_comment.as_bytes()),
            "erased file comment survived in output (with_survivor={with_survivor}, randomize={randomize})"
        );

        let mut archive = ZipArchive::new(Cursor::new(erased))?;
        assert_eq!(archive.len(), usize::from(with_survivor));
        if with_survivor {
            let mut s = String::new();
            archive.by_name("kept.txt")?.read_to_string(&mut s)?;
            assert_eq!(s, "still here");
        }
    }
    Ok(())
}

#[test]
fn erase_file_allows_adjacent_preceding_dd_and_rejects_backward_overlap() -> ZipResult<()> {
    // 1. Preceding entry has a 16-byte DD ending right at `second.txt`'s header_start: must succeed.
    let adjacent = build_custom_dd_zip(&[
        ("first.txt", b"first payload", Some(true)),
        ("second.txt", b"second payload", None),
    ]);
    let mut zip = ZipWriter::new_append(Cursor::new(adjacent))?;
    zip.erase_file("second.txt", false)?;
    let mut archive = ZipArchive::new(zip.finish()?)?;
    assert_eq!(archive.len(), 1);
    let mut s = String::new();
    archive.by_name("first.txt")?.read_to_string(&mut s)?;
    assert_eq!(s, "first payload");

    // 2. Preceding entry's payload overlaps `inner.txt`'s local header: must return Err without
    // corrupting `outer.bin`.
    let inner_payload = b"inner data";
    let inner_crc = ieee_crc32(inner_payload);
    let mut outer_payload = Vec::new();
    // Embed a valid local file header + payload for "inner.txt" inside outer_payload!
    let inner_rel_offset = 4usize;
    outer_payload.extend_from_slice(b"PRE_");
    outer_payload.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    outer_payload.extend_from_slice(&20u16.to_le_bytes());
    outer_payload.extend_from_slice(&0u16.to_le_bytes());
    outer_payload.extend_from_slice(&0u16.to_le_bytes());
    outer_payload.extend_from_slice(&0u16.to_le_bytes());
    outer_payload.extend_from_slice(&0u16.to_le_bytes());
    outer_payload.extend_from_slice(&inner_crc.to_le_bytes());
    outer_payload.extend_from_slice(&(inner_payload.len() as u32).to_le_bytes());
    outer_payload.extend_from_slice(&(inner_payload.len() as u32).to_le_bytes());
    outer_payload.extend_from_slice(&(b"inner.txt".len() as u16).to_le_bytes());
    outer_payload.extend_from_slice(&0u16.to_le_bytes());
    outer_payload.extend_from_slice(b"inner.txt");
    outer_payload.extend_from_slice(inner_payload);
    outer_payload.extend_from_slice(b"_POST");

    let outer_crc = ieee_crc32(&outer_payload);
    let mut buf = Vec::new();
    // Local file header for "outer.bin" at offset 0.
    buf.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    buf.extend_from_slice(&20u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&outer_crc.to_le_bytes());
    buf.extend_from_slice(&(outer_payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(outer_payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(b"outer.bin".len() as u16).to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(b"outer.bin");
    let outer_data_start = buf.len();
    buf.extend_from_slice(&outer_payload);

    let inner_header_start = (outer_data_start + inner_rel_offset) as u32;
    let cd_start = buf.len() as u32;
    let mut cd = Vec::new();
    for &(name, crc, size, offset) in &[
        ("outer.bin", outer_crc, outer_payload.len() as u32, 0u32),
        (
            "inner.txt",
            inner_crc,
            inner_payload.len() as u32,
            inner_header_start,
        ),
    ] {
        cd.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        cd.extend_from_slice(&20u16.to_le_bytes());
        cd.extend_from_slice(&20u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&crc.to_le_bytes());
        cd.extend_from_slice(&size.to_le_bytes());
        cd.extend_from_slice(&size.to_le_bytes());
        cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u32.to_le_bytes());
        cd.extend_from_slice(&offset.to_le_bytes());
        cd.extend_from_slice(name.as_bytes());
    }
    buf.extend_from_slice(&cd);
    buf.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes());
    buf.extend_from_slice(&(cd.len() as u32).to_le_bytes());
    buf.extend_from_slice(&cd_start.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());

    let mut zip = ZipWriter::new_append(Cursor::new(buf))?;
    assert!(zip.erase_file("inner.txt", false).is_err());

    let mut archive = ZipArchive::new(zip.finish()?)?;
    assert_eq!(archive.len(), 2);
    let mut got_outer = Vec::new();
    archive.by_name("outer.bin")?.read_to_end(&mut got_outer)?;
    assert_eq!(got_outer, outer_payload);
    Ok(())
}
