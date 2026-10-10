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
