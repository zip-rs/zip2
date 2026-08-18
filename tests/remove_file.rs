//! Test aobut removing file

use std::io::{Cursor, Write};

use zip::CompressionMethod;
use zip::ZipArchive;
use zip::ZipWriter;
use zip::result::{ZipError, ZipResult};
use zip::write::SimpleFileOptions;

#[test]
fn remove_file_drops_the_entry_but_keeps_the_others() -> ZipResult<()> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for name in ["a.txt", "b.txt", "c.txt"] {
        zip.start_file(name, SimpleFileOptions::default())?;
        zip.write_all(name.as_bytes())?;
    }
    zip.soft_remove_file("b.txt")?;

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
fn remove_file_preserves_the_order_of_surviving_entries() -> ZipResult<()> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        zip.start_file(name, SimpleFileOptions::default())?;
        zip.write_all(b"x")?;
    }
    zip.soft_remove_file("b.txt")?;

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
fn remove_file_does_not_rewrite_the_archive() -> ZipResult<()> {
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
        zip.soft_remove_file("gone.txt")?;
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
fn remove_file_frees_the_name_for_reuse() -> ZipResult<()> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("a.txt", SimpleFileOptions::default())?;
    zip.write_all(b"first")?;
    zip.soft_remove_file("a.txt")?;
    zip.start_file("a.txt", SimpleFileOptions::default())?;
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
fn remove_file_can_remove_the_entry_being_written() -> ZipResult<()> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("a.txt", SimpleFileOptions::default())?;
    zip.write_all(b"a")?;
    zip.start_file("b.txt", SimpleFileOptions::default())?;
    zip.write_all(b"b")?;
    zip.soft_remove_file("b.txt")?;

    let mut archive = ZipArchive::new(zip.finish()?)?;
    assert_eq!(archive.len(), 1);
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("a.txt")?, &mut contents)?;
    assert_eq!(contents, "a");
    Ok(())
}

#[test]
fn remove_file_reports_a_missing_entry() {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    assert!(matches!(
        zip.soft_remove_file("absent.txt"),
        Err(ZipError::FileNotFound)
    ));
}

/// An entry sharing data with the removed one keeps working: removal
/// touches the directory, never the bytes.
#[test]
fn remove_file_leaves_a_shallow_copy_readable() -> ZipResult<()> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("original.txt", SimpleFileOptions::default())?;
    zip.write_all(b"shared")?;
    zip.shallow_copy_file("original.txt", "copy.txt")?;
    zip.soft_remove_file("original.txt")?;

    let mut archive = ZipArchive::new(zip.finish()?)?;
    assert_eq!(archive.len(), 1);
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("copy.txt")?, &mut contents)?;
    assert_eq!(contents, "shared");
    Ok(())
}
