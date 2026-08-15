//! Zip stream tests

use zip::read::ZipFile;
use zip::read::ZipFileEntry;
use zip::unstable::stream::{ZipStreamReader, ZipStreamVisitor};
use zip::result::ZipResult;
use std::io::{Cursor, Read};

struct DummyVisitor;
impl ZipStreamVisitor for DummyVisitor {
    fn visit_file<R: Read>(&mut self, _file: &mut ZipFile<'_, R>) -> ZipResult<()> {
        Ok(())
    }

    fn visit_additional_metadata(&mut self, _metadata: &ZipFileEntry<'_>) -> ZipResult<()> {
        Ok(())
    }
}

#[allow(dead_code)]
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
    ZipStreamReader::new(Cursor::new(include_bytes!(
        "data/invalid_offset.zip"
    )))
    .visit(&mut DummyVisitor)
    .unwrap_err();
}

#[test]
fn invalid_offset2() {
    ZipStreamReader::new(Cursor::new(include_bytes!(
        "data/invalid_offset2.zip"
    )))
    .visit(&mut DummyVisitor)
    .unwrap_err();
}
