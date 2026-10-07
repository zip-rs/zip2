//! Zip stream tests

use std::io::{Cursor, Read};
use zip::read::ZipFile;
use zip::read::ZipFileEntry;
use zip::result::ZipResult;
use zip::unstable::stream::{ZipStreamReader, ZipStreamVisitor};

struct DummyVisitor;
impl ZipStreamVisitor for DummyVisitor {
    fn visit_file<R: Read>(&mut self, _file: &mut ZipFile<'_, R>) -> ZipResult<()> {
        Ok(())
    }

    fn visit_additional_metadata(&mut self, _metadata: &ZipFileEntry<'_>) -> ZipResult<()> {
        Ok(())
    }
}

#[derive(Default, Debug, Eq, PartialEq)]
struct CounterVisitor(u64, u64);

impl ZipStreamVisitor for CounterVisitor {
    fn visit_file<R: Read>(&mut self, _file: &mut ZipFile<'_, R>) -> ZipResult<()> {
        self.0 += 1;
        Ok(())
    }

    fn visit_additional_metadata(&mut self, _metadata: &ZipFileEntry<'_>) -> ZipResult<()> {
        self.1 += 1;
        Ok(())
    }
}

#[test]
fn invalid_offset() {
    ZipStreamReader::new(Cursor::new(include_bytes!("data/invalid_offset.zip")))
        .visit(&mut DummyVisitor)
        .unwrap_err();
}

#[test]
fn invalid_offset2() {
    ZipStreamReader::new(Cursor::new(include_bytes!("data/invalid_offset2.zip")))
        .visit(&mut DummyVisitor)
        .unwrap_err();
}

fn three_files_with_modes() -> Vec<u8> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    // Stored: the stream reader must decode these, and a zopfli-only build can't inflate.
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, mode) in [("a.sh", 0o751), ("b.txt", 0o600), ("c.txt", 0o644)] {
        w.start_file(name, opts.unix_permissions(mode)).unwrap();
        w.write_all(name.as_bytes()).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// `visit` reads every central directory header after the local entries. The local-entry loop
/// stops on the central directory signature, so the first header must still parse.
#[test]
fn visit_sees_every_central_directory_entry() {
    let mut counter = CounterVisitor::default();
    ZipStreamReader::new(Cursor::new(three_files_with_modes()))
        .visit(&mut counter)
        .unwrap();
    assert_eq!(counter, CounterVisitor(3, 3));
}

/// `extract` applies the unix modes from the central directory, as `ZipArchive::extract` does.
// These tests use the file system, which miri's isolation does not allow.
#[cfg(all(unix, not(miri)))]
#[test]
fn extract_applies_unix_modes() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::TempDir::new().unwrap();
    ZipStreamReader::new(Cursor::new(three_files_with_modes()))
        .extract(dir.path())
        .unwrap();
    for (name, mode) in [("a.sh", 0o751), ("b.txt", 0o600), ("c.txt", 0o644)] {
        let got = std::fs::metadata(dir.path().join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(got, mode, "{name}");
    }
}

/// The central directory is not checked against the local headers, so it can name a path that no
/// local entry wrote. Sets up `dest/<link>` as a symlink to `link_target` (relative to the base,
/// which also holds `outside/b.txt` with mode 600), extracts one entry whose central directory
/// name is `central_name` with mode 777 while its local header says `local_name` (same length),
/// and returns the mode of `outside/b.txt` afterwards.
#[cfg(all(unix, not(miri)))]
fn victim_mode_after_central_only_name(
    link: &str,
    link_target: &str,
    central_name: &str,
    local_name: &str,
) -> u32 {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use zip::write::SimpleFileOptions;

    let base = tempfile::TempDir::new().unwrap();
    let outside = base.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let victim = outside.join("b.txt");
    std::fs::write(&victim, b"keep").unwrap();
    std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o600)).unwrap();
    let dest = base.path().join("dest");
    std::fs::create_dir(&dest).unwrap();
    std::os::unix::fs::symlink(base.path().join(link_target), dest.join(link)).unwrap();

    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o777);
    w.start_file(central_name, opts).unwrap();
    w.write_all(b"x").unwrap();
    let mut archive = w.finish().unwrap().into_inner();
    // Rename the entry in its local header only, so the stream writes `local_name` while the
    // central directory still gives `central_name` mode 777.
    let at = 30;
    let len = central_name.len();
    assert_eq!(local_name.len(), len);
    assert_eq!(&archive[at..at + len], central_name.as_bytes());
    archive[at..at + len].copy_from_slice(local_name.as_bytes());

    ZipStreamReader::new(Cursor::new(archive))
        .extract(&dest)
        .unwrap();
    assert!(
        dest.join(local_name).is_file(),
        "the local entry was not written"
    );
    std::fs::metadata(&victim).unwrap().permissions().mode() & 0o777
}

/// A central-directory-only name that goes through a symlink already in the destination, to a
/// file outside it, must get no mode.
#[cfg(all(unix, not(miri)))]
#[test]
fn extract_ignores_modes_for_names_it_did_not_write() {
    let mode = victim_mode_after_central_only_name("a", "outside", "a/b.txt", "c_d.txt");
    assert_eq!(mode, 0o600, "a mode was applied through the symlink");
}

/// The same when the central-directory-only name is the symlink itself: `set_permissions` would
/// follow it to the file outside.
#[cfg(all(unix, not(miri)))]
#[test]
fn extract_ignores_modes_for_a_name_that_is_a_symlink_it_did_not_write() {
    let mode = victim_mode_after_central_only_name("e.txt", "outside/b.txt", "e.txt", "f.txt");
    assert_eq!(mode, 0o600, "a mode was applied through the symlink");
}

/// test case to ensure we don't preemptively over allocate based on the
/// declared number of files in the CDE of an invalid zip when the number of
/// files declared is more than the alleged offset in the CDE
#[test]
fn invalid_cde_number_of_files_allocation_smaller_offset() {
    ZipStreamReader::new(Cursor::new(include_bytes!(
        "data/invalid_cde_number_of_files_allocation_smaller_offset.zip"
    )))
    .visit(&mut DummyVisitor)
    .unwrap_err();
}

/// test case to ensure we don't preemptively over allocate based on the
/// declared number of files in the CDE of an invalid zip when the number of
/// files declared is less than the alleged offset in the CDE
#[test]
fn invalid_cde_number_of_files_allocation_greater_offset() {
    ZipStreamReader::new(Cursor::new(include_bytes!(
        "data/invalid_cde_number_of_files_allocation_greater_offset.zip"
    )))
    .visit(&mut DummyVisitor)
    .unwrap_err();
}
