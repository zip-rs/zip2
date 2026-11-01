#![cfg(all(windows, not(miri)))]

use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;
use std::process::Command;

use tempfile::tempdir;
use zip::unstable::stream::ZipStreamReader;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipStreamExtract, ZipWriter};

fn archive(names: &[&str]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for name in names {
        writer
            .start_file(*name, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"replacement").unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn junction(link: &Path, target: &Path) {
    let status = Command::new("cmd.exe")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .status()
        .unwrap();
    assert!(status.success(), "could not create test junction: {status}");
}

#[test]
fn stream_rejects_a_junction_alias_before_committing_files() {
    let root = tempdir().unwrap();
    let dest = root.path().join("dest");
    fs::create_dir(&dest).unwrap();
    junction(&dest.join("alias"), &dest);

    let result = ZipStreamReader::new(Cursor::new(archive(&["a.txt", "alias/a.txt"])))
        .enforce_header_consistency(true)
        .extract(&dest);
    assert!(result.is_err());
    assert!(!dest.join("a.txt").exists());
}

#[test]
fn stream_rejects_a_junction_outside_the_destination() {
    let root = tempdir().unwrap();
    let dest = root.path().join("dest");
    let outside = root.path().join("outside");
    fs::create_dir(&dest).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("victim.txt"), b"original").unwrap();
    junction(&dest.join("alias"), &outside);

    let bytes = archive(&["alias/victim.txt"]);
    let result = ZipStreamReader::new(Cursor::new(&bytes))
        .enforce_header_consistency(true)
        .extract(&dest);
    assert!(result.is_err());
    assert_eq!(fs::read(outside.join("victim.txt")).unwrap(), b"original");

    let result = ZipArchive::new(Cursor::new(&bytes)).unwrap().extract(&dest);
    assert!(result.is_err());
    assert_eq!(fs::read(outside.join("victim.txt")).unwrap(), b"original");
}

use zip::ZipExtract;
