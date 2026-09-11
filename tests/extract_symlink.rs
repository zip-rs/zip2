/// Only on little endian because we cannot use fs with miri CI
#[cfg(all(target_endian = "little", not(miri)))]
#[test]
fn test_is_symlink() -> std::io::Result<()> {
    use std::io::Cursor;
    use tempfile::TempDir;
    use zip::ZipArchive;

    let mut reader = ZipArchive::new(Cursor::new(include_bytes!("../tests/data/symlink.zip")))?;
    assert!(reader.by_index(0)?.is_symlink());
    let tempdir = TempDir::with_prefix("test_is_symlink")?;
    reader.extract(&tempdir)?;
    assert!(tempdir.path().join("bar").is_symlink());
    Ok(())
}

#[test]
#[cfg(all(unix, feature = "deflate-flate2"))]
fn extract_should_respect_links() {
    use std::{fs, io, path::PathBuf, str::FromStr};
    use tempfile::TempDir;
    use zip::ZipArchive;

    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/pandoc_soft_links.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");
    let temp_dir = TempDir::with_prefix("pandoc_soft_links").unwrap();
    archive.extract(&temp_dir).unwrap();

    let symlink_path = temp_dir.path().join("pandoc-3.2-arm64/bin/pandoc-lua");

    // Read the target of the symbolic link
    let target_path = fs::read_link(&symlink_path).unwrap();

    assert_eq!(target_path, PathBuf::from_str("pandoc").unwrap());
}

/// A symlink entry whose central directory declares an absurd uncompressed size
/// must be rejected, not pre-allocated.
///
/// The declared size comes from the archive itself. Before the fix, `extract`
/// passed it straight to `Vec::with_capacity`, so a hostile entry could ask for
/// 2^62 bytes and abort the whole process ("memory allocation of ... failed").
/// A symlink target is a path, so a size this large is malformed and the entry
/// must come back as an error.
#[cfg(all(target_pointer_width = "64", target_endian = "little", not(miri)))]
#[test]
fn extract_symlink_with_hostile_declared_size() -> zip::result::ZipResult<()> {
    use std::io::Cursor;
    use tempfile::TempDir;
    use zip::ZipArchive;

    /// One entry, flagged as a symlink, whose Zip64 extra field claims
    /// `declared_size` uncompressed bytes while storing only `payload`.
    fn build(declared_size: u64, payload: &[u8]) -> Vec<u8> {
        const S_IFLNK: u32 = 0o120_000;
        const SENTINEL: u32 = 0xFFFF_FFFF;
        let name = b"link";
        let mut o = Vec::new();

        // local file header
        o.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
        o.extend_from_slice(&45u16.to_le_bytes()); // version needed: zip64
        o.extend_from_slice(&0u16.to_le_bytes()); // flags
        o.extend_from_slice(&0u16.to_le_bytes()); // method: Stored
        o.extend_from_slice(&0u16.to_le_bytes()); // mod time
        o.extend_from_slice(&0x21u16.to_le_bytes()); // mod date
        o.extend_from_slice(&0u32.to_le_bytes()); // crc32
        o.extend_from_slice(&SENTINEL.to_le_bytes());
        o.extend_from_slice(&SENTINEL.to_le_bytes());
        o.extend_from_slice(&(name.len() as u16).to_le_bytes());
        o.extend_from_slice(&20u16.to_le_bytes()); // zip64 extra field
        o.extend_from_slice(name);
        o.extend_from_slice(&0x0001u16.to_le_bytes()); // zip64 header id
        o.extend_from_slice(&16u16.to_le_bytes());
        o.extend_from_slice(&declared_size.to_le_bytes());
        o.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        o.extend_from_slice(payload);

        let cd_offset = o.len() as u32;

        // central directory header
        o.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
        o.extend_from_slice(&0x031Eu16.to_le_bytes()); // version made by: Unix
        o.extend_from_slice(&45u16.to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        o.extend_from_slice(&0x21u16.to_le_bytes());
        o.extend_from_slice(&0u32.to_le_bytes()); // crc32
        o.extend_from_slice(&SENTINEL.to_le_bytes());
        o.extend_from_slice(&SENTINEL.to_le_bytes());
        o.extend_from_slice(&(name.len() as u16).to_le_bytes());
        o.extend_from_slice(&20u16.to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes()); // comment len
        o.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        o.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        o.extend_from_slice(&((S_IFLNK | 0o777) << 16).to_le_bytes()); // symlink
        o.extend_from_slice(&0u32.to_le_bytes()); // local header offset
        o.extend_from_slice(name);
        o.extend_from_slice(&0x0001u16.to_le_bytes());
        o.extend_from_slice(&16u16.to_le_bytes());
        o.extend_from_slice(&declared_size.to_le_bytes());
        o.extend_from_slice(&(payload.len() as u64).to_le_bytes());

        let cd_size = o.len() as u32 - cd_offset;

        // end of central directory
        o.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        o.extend_from_slice(&0u16.to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        o.extend_from_slice(&1u16.to_le_bytes());
        o.extend_from_slice(&1u16.to_le_bytes());
        o.extend_from_slice(&cd_size.to_le_bytes());
        o.extend_from_slice(&cd_offset.to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes());
        o
    }

    for declared_size in [1u64 << 40, 1u64 << 62, u64::MAX] {
        let mut reader = ZipArchive::new(Cursor::new(build(declared_size, b"/etc/passwd")))?;
        assert!(reader.by_index(0)?.is_symlink());
        let tempdir = TempDir::with_prefix("test_hostile_symlink_size")?;

        let err = reader
            .extract(&tempdir)
            .expect_err("oversized symlink target must be rejected");
        assert!(
            matches!(err, zip::result::ZipError::InvalidArchive(_)),
            "expected InvalidArchive for a {declared_size} byte symlink target, got {err:?}"
        );
    }

    Ok(())
}

/// Symlinks being extracted shouldn't be followed out of the destination directory.
/// Only on little endian because we cannot use fs with miri CI
#[cfg(all(target_endian = "little", not(miri)))]
#[test]
fn test_cannot_symlink_outside_destination() -> zip::result::ZipResult<()> {
    use std::fs::create_dir;
    use std::io::Cursor;
    use tempfile::TempDir;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.add_symlink("symlink/", "../dest-sibling/", SimpleFileOptions::default())?;
    writer.start_file("symlink/dest-file", SimpleFileOptions::default())?;
    let mut reader = writer.finish_into_readable()?;
    let dest_parent = TempDir::with_prefix("read__test_cannot_symlink_outside_destination")?;
    let dest_sibling = dest_parent.path().join("dest-sibling");
    create_dir(&dest_sibling)?;
    let dest = dest_parent.path().join("dest");
    create_dir(&dest)?;
    assert!(reader.extract(dest).is_err());
    assert!(!dest_sibling.join("dest-file").exists());
    Ok(())
}
