//! On Windows `c:x` is relative to drive C:'s current directory and replaces the path it's
//! joined to, so no `c:x`-style component may come out of entry-name handling.

use std::io::{Cursor, Write};
use std::path::Path;

use typed_path::{Utf8WindowsComponent, Utf8WindowsPath};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipExtract, ZipFilePathExt, ZipWriter};

const NAMES: &[&str] = &["/c:x", "./c:x", "a/../c:x", "/c:x/y", "b/c:x", "/c:d:x"];

fn archive(names: &[&str]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for name in names {
        writer.start_file(*name, options).unwrap();
        writer.write_all(b"data").unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// Each component of `path` parses under Windows rules as exactly one normal component.
fn assert_windows_relative(name: &str, path: &Path) {
    for component in path.iter() {
        let component = component.to_str().unwrap();
        let parsed: Vec<_> = Utf8WindowsPath::new(component).components().collect();
        assert_eq!(
            parsed,
            [Utf8WindowsComponent::Normal(component)],
            "{name:?} -> {path:?}: {component:?} parses as {parsed:?} on Windows"
        );
    }
}

#[test]
fn enclosed_and_mangled_names_have_no_drive_prefix_inside() {
    let mut archive = ZipArchive::new(Cursor::new(archive(NAMES))).unwrap();
    for i in 0..archive.len() {
        let file = archive.by_index(i).unwrap();
        let name = file.name().unwrap().into_owned();
        if let Some(enclosed) = file.enclosed_name() {
            assert_windows_relative(&name, &enclosed);
        }
        assert_windows_relative(&name, &file.mangled_name().unwrap());
    }
}

/// `a/../c:foo/a` used to hit a debug assertion, and `a/../c:/a` extracted to `dest/c:/a`.
#[cfg(not(miri))]
#[test]
fn unwrapped_root_dir_named_like_a_drive() {
    for (name, expected) in [("a/../c:foo/a", "a"), ("a/../c:/a", "a")] {
        let dest = tempfile::TempDir::new().unwrap();
        let mut archive = ZipArchive::new(Cursor::new(archive(&[name]))).unwrap();
        archive
            .extract_unwrapped_root_dir(dest.path(), zip::read::root_dir_common_filter)
            .unwrap();
        assert!(dest.path().join(expected).is_file(), "{name}");
        let root = archive.root_dir(zip::read::root_dir_common_filter).unwrap();
        if let Some(root) = root {
            assert_windows_relative(name, &root);
        }
    }
}

/// What a caller does with the documented-as-safe name: join it onto a destination.
#[cfg(windows)]
#[test]
fn joined_enclosed_and_mangled_names_stay_under_the_destination() {
    let dest = Path::new(r"C:\dest");
    let mut archive = ZipArchive::new(Cursor::new(archive(NAMES))).unwrap();
    for i in 0..archive.len() {
        let file = archive.by_index(i).unwrap();
        let name = file.name().unwrap().into_owned();
        if let Some(enclosed) = file.enclosed_name() {
            let joined = dest.join(&enclosed);
            assert!(joined.starts_with(dest), "{name:?} -> {joined:?}");
        }
        let joined = dest.join(file.mangled_name().unwrap());
        assert!(joined.starts_with(dest), "{name:?} -> mangled {joined:?}");
    }
}

/// `extract` must not create anything outside the destination, not even directories.
#[cfg(not(miri))]
#[test]
fn extract_creates_nothing_outside_the_destination() {
    use std::path::{Component, Prefix};

    let cwd = std::env::current_dir().unwrap();
    let drive = match cwd.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => d as char,
            other => panic!("current directory has no drive letter: {other:?}"),
        },
        // No drives off Windows; any letter will do.
        _ if cfg!(not(windows)) => 'c',
        other => panic!("current directory has no prefix: {other:?}"),
    };
    let escaped = format!("zip2-drive-component-{}", std::process::id());
    let outside = cwd.join(&escaped);
    let _ = std::fs::remove_dir_all(&outside);

    let names = [
        format!("/{drive}:{escaped}/sub/file.txt"),
        format!("b/{drive}:{escaped}/sub/file.txt"),
    ];
    for name in &names {
        let dest = tempfile::TempDir::new().unwrap();
        let mut archive = ZipArchive::new(Cursor::new(archive(&[name.as_str()]))).unwrap();
        let result = archive.extract(dest.path());
        let created_outside = outside.exists();
        let _ = std::fs::remove_dir_all(&outside);
        assert!(
            !created_outside,
            "{name:?} created {outside:?} outside the destination (extract returned {result:?})"
        );
        if let Ok(()) = result {
            let mut found = Vec::new();
            for entry in walkdir::WalkDir::new(dest.path()) {
                found.push(entry.unwrap().into_path());
            }
            assert!(
                found.iter().any(|p| p.ends_with("sub/file.txt")),
                "{name:?} extracted to {found:?}"
            );
        }
    }
}
