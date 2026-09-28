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
#[cfg(not(miri))]
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

/// A symlink target that is only *lexically* inside the destination must not be usable
/// to escape it: `a -> <dest>/b/sub` passes a lexical prefix check, but resolving it
/// traverses `b`, which points out of the destination.
///
/// Regression test for GHSA-hjqx-j4rw-g7xv.
/// Only on native platforms because we cannot use fs with miri CI
#[cfg(not(miri))]
#[test]
fn zip_slip_via_symlink() -> zip::result::ZipResult<()> {
    use std::fs::create_dir;
    use std::io::{Cursor, Write};
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    let tmp = TempDir::with_prefix("read__zip_slip_via_symlink")?;
    // The attack needs the real path of the destination, so canonicalize it here as well.
    let tmp_path = tmp.path().canonicalize()?;
    let dest = tmp_path.join("dest");
    let outside = tmp_path.join("outside");
    create_dir(&dest)?;
    create_dir(&outside)?;

    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    // `b` points out of the destination directory.
    writer.add_symlink("b", outside.to_str().unwrap(), options)?;
    // `a` is prefixed with the destination directory, so a lexical check accepts it,
    // but it resolves through `b` and therefore leaves the destination directory.
    writer.add_symlink("a", dest.join("b/sub").to_str().unwrap(), options)?;
    // Written through `a`, so the kernel follows `b` and escapes.
    writer.start_file("a/pwned.txt", options)?;
    writer.write_all(b"escaped!")?;
    let mut reader = writer.finish_into_readable()?;

    assert!(reader.extract(&dest).is_err());
    assert!(!outside.join("sub").exists());
    assert!(!outside.join("sub/pwned.txt").exists());
    Ok(())
}

/// The same escape must not be able to overwrite an existing file outside the
/// destination directory, such as `~/.ssh/authorized_keys`.
///
/// Regression test for GHSA-hjqx-j4rw-g7xv.
/// Only on native platforms because we cannot use fs with miri CI
#[cfg(not(miri))]
#[test]
fn overwrite_existing_ssh_authorized_keys() -> zip::result::ZipResult<()> {
    use std::fs::{create_dir, create_dir_all, read_to_string, write};
    use std::io::{Cursor, Write};
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    const LEGIT_KEY: &str = "ssh-ed25519 AAAA...LEGIT victim@host\n";

    let tmp = TempDir::with_prefix("read__overwrite_existing_ssh_authorized_keys")?;
    let tmp_path = tmp.path().canonicalize()?;
    let dest = tmp_path.join("dest");
    create_dir(&dest)?;

    // An existing file outside the destination directory.
    let home = tmp_path.join("home_victim");
    let authorized_keys = home.join(".ssh/authorized_keys");
    create_dir_all(home.join(".ssh"))?;
    write(&authorized_keys, LEGIT_KEY)?;

    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    // `b` points out of the destination directory, at the victim's home directory.
    writer.add_symlink("b", home.to_str().unwrap(), options)?;
    // `a` is prefixed with the destination directory, but resolves through `b`.
    writer.add_symlink("a", dest.join("b/.ssh").to_str().unwrap(), options)?;
    writer.start_file("a/authorized_keys", options)?;
    writer.write_all(b"ssh-ed25519 AAAA...ATTACKER attacker@evil\n")?;
    let mut reader = writer.finish_into_readable()?;

    assert!(reader.extract(&dest).is_err());
    assert_eq!(read_to_string(&authorized_keys)?, LEGIT_KEY);
    Ok(())
}

/// A symlink whose target leaves the destination directory must be rejected when the
/// symlink is created, even when nothing in the archive traverses it afterwards.
/// Otherwise it is left on disk for a later extraction into the same directory to follow.
///
/// Regression test for GHSA-hjqx-j4rw-g7xv.
/// Only on native platforms because we cannot use fs with miri CI
#[cfg(not(miri))]
#[test]
fn test_cannot_create_symlink_pointing_outside_destination() -> zip::result::ZipResult<()> {
    use std::fs::create_dir;
    use std::io::Cursor;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    let tmp =
        TempDir::with_prefix("read__test_cannot_create_symlink_pointing_outside_destination")?;
    let tmp_path = tmp.path().canonicalize()?;
    let dest = tmp_path.join("dest");
    let outside = tmp_path.join("outside");
    create_dir(&dest)?;
    create_dir(&outside)?;

    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for target in [outside.to_str().unwrap(), "../outside"] {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.add_symlink("b", target, options)?;
        let mut reader = writer.finish_into_readable()?;

        assert!(
            reader.extract(&dest).is_err(),
            "extraction accepted a symlink to {target}"
        );
        // `exists()` would follow the link, so look at the link itself.
        assert!(
            dest.join("b").symlink_metadata().is_err(),
            "a symlink to {target} was left on disk"
        );
    }
    Ok(())
}

/// The escape must also be blocked when the outward symlink is reached through a longer
/// chain: `a` resolves to `<dest>/b/sub`, `b` resolves to `<dest>/c`, and only `c` points
/// out of the destination. Every hop has to be re-checked, not just the first one.
///
/// Regression test for GHSA-hjqx-j4rw-g7xv.
/// Only on native platforms because we cannot use fs with miri CI
#[cfg(not(miri))]
#[test]
fn test_cannot_escape_through_a_symlink_chain() -> zip::result::ZipResult<()> {
    use std::fs::create_dir;
    use std::io::{Cursor, Write};
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    let tmp = TempDir::with_prefix("read__test_cannot_escape_through_a_symlink_chain")?;
    let tmp_path = tmp.path().canonicalize()?;
    let dest = tmp_path.join("dest");
    let outside = tmp_path.join("outside");
    create_dir(&dest)?;
    create_dir(&outside)?;

    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    // Only `c` leaves the destination directory.
    writer.add_symlink("c", outside.to_str().unwrap(), options)?;
    // `b` and `a` are lexically inside it, and each one exists by the time the next is
    // resolved, so the whole chain resolves to a directory outside the destination.
    writer.add_symlink("b", dest.join("c").to_str().unwrap(), options)?;
    writer.add_symlink("a", dest.join("b/sub").to_str().unwrap(), options)?;
    writer.start_file("a/pwned.txt", options)?;
    writer.write_all(b"escaped!")?;
    let mut reader = writer.finish_into_readable()?;

    assert!(reader.extract(&dest).is_err());
    assert!(!outside.join("sub").exists());
    assert!(!outside.join("sub/pwned.txt").exists());
    Ok(())
}

/// A ZIP archive can spell a symlink target the Windows way — verbatim (`\\?\C:\...`),
/// device (`\\.\C:\...`), UNC in both its verbatim (`\\?\UNC\server\share\...`) and
/// plain (`\\server\share\...`) form, plain drive (`C:\...`), drive-relative (`C:evil`)
/// or root-relative (`\evil`) — no matter which platform wrote it, so the extracting
/// side has to hold up everywhere. On Windows the absolute ones are compared against
/// the canonicalized destination and the rest are refused outright, since the OS would
/// resolve them from somewhere other than the link's parent. On Unix none of them is
/// absolute, so each is a single relative name. Either way the link must not end up
/// pointing outside the destination.
///
/// Regression test for GHSA-hjqx-j4rw-g7xv.
/// Only on native platforms because we cannot use fs with miri CI
#[cfg(not(miri))]
#[test]
fn test_symlink_target_with_verbatim_prefix_cannot_escape() -> zip::result::ZipResult<()> {
    use std::fs::{create_dir, read_dir, read_link};
    use std::io::{Cursor, Write};
    use std::path::Path;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    /// Build an archive whose `link` entry is a symlink to `target`, with a file written
    /// through it, and extract it into a fresh destination directory.
    fn extract_symlink_to(
        parent: &Path,
        name: &str,
        target: &str,
    ) -> zip::result::ZipResult<(std::path::PathBuf, zip::result::ZipResult<()>)> {
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.add_symlink("link", target, options)?;
        writer.start_file("link/pwned.txt", options)?;
        writer.write_all(b"escaped!")?;
        let mut reader = writer.finish_into_readable()?;

        let dest = parent.join(name);
        create_dir(&dest)?;
        let result = reader.extract(&dest);
        Ok((dest, result))
    }

    let tmp = TempDir::with_prefix("read__symlink_target_with_verbatim_prefix")?;
    let tmp_path = tmp.path().canonicalize()?;
    let outside = tmp_path.join("outside");
    create_dir(&outside)?;

    // A real directory outside the destination, spelled both the way `canonicalize`
    // spells it (verbatim on Windows) and without that prefix. The second form is the
    // one an archive would realistically carry, and the one the verbatim-vs-ordinary
    // comparison exists for; both have to be rejected.
    let outside_str = outside.to_str().unwrap().to_owned();
    let outside_plain = outside_str
        .strip_prefix(r"\\?\")
        .unwrap_or(&outside_str)
        .to_owned();

    for (i, target) in [&outside_str, &outside_plain].iter().enumerate() {
        let (_dest, result) = extract_symlink_to(&tmp_path, &format!("dest-outside-{i}"), target)?;
        assert!(
            result.is_err(),
            "a symlink to {target} should not be extractable"
        );
        assert!(!outside.join("pwned.txt").exists());
        assert_eq!(
            read_dir(&outside)?.count(),
            0,
            "{target} wrote into {outside:?}"
        );
    }

    // Targets an archive can carry regardless of where it was written. None of these
    // name anything inside the destination, so extraction either fails or leaves a link
    // whose target is still inside it (on Unix they are ordinary relative file names).
    for (i, target) in [
        r"\\?\C:\Windows\System32\evil",
        r"\\?\UNC\server\share\evil",
        r"\\server\share\evil",
        r"\\.\C:\evil",
        r"C:\Windows\evil",
        r"\evil",
        r"C:evil",
    ]
    .iter()
    .enumerate()
    {
        let (dest, result) = extract_symlink_to(&tmp_path, &format!("dest-fixed-{i}"), target)?;
        let link = dest.join("link");
        // Either the target was rejected outright (what a Windows host does with the
        // absolute ones), or a link was created inside the destination.
        assert!(
            result.is_err() || link.symlink_metadata().is_ok(),
            "{target} neither was rejected nor produced a link in {dest:?}"
        );
        if let Ok(link_target) = read_link(&link) {
            // None of these targets contain `..`, so a lexical check is enough here.
            let resolved = if link_target.is_absolute() {
                link_target
            } else {
                link.parent().unwrap().join(link_target)
            };
            assert!(
                resolved.starts_with(&dest),
                "{target} resolved to {resolved:?}, outside {dest:?}"
            );
        }
    }

    Ok(())
}

/// Symlinks being extracted shouldn't be followed out of the destination directory.
/// Only on native platforms because we cannot use fs with miri CI
#[test]
#[cfg(not(miri))]
fn test_cannot_symlink_outside_destination_zip_stream() {
    use std::fs::create_dir;
    use std::io::Cursor;
    use tempfile::TempDir;
    use zip::ZipWriter;
    use zip::unstable::stream::ZipStreamReader;
    use zip::write::SimpleFileOptions;

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .add_symlink("symlink/", "../dest-sibling/", SimpleFileOptions::default())
        .unwrap();
    writer
        .start_file("symlink/dest-file", SimpleFileOptions::default())
        .unwrap();
    let reader = ZipStreamReader::new(writer.finish().unwrap());
    let dest_parent = TempDir::with_prefix("stream__cannot_symlink_outside_destination").unwrap();
    let dest_sibling = dest_parent.path().join("dest-sibling");
    create_dir(&dest_sibling).unwrap();
    let dest = dest_parent.path().join("dest");
    create_dir(&dest).unwrap();
    assert!(reader.extract(dest).is_err());
    assert!(!dest_sibling.join("dest-file").exists());
}
