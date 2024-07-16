use std::fs;
use std::io::{self, Read};
use std::path::Path;

use super::{ZipError, ZipResult};
use crate::unstable::read::{
    streaming::{StreamingArchive, StreamingZipEntry, ZipStreamFileMetadata},
    ArchiveEntry,
};

/// Stream decoder for zip.
#[derive(Debug)]
pub struct ZipStreamReader<R>(StreamingArchive<R>);

impl<R> ZipStreamReader<R> {
    /// Create a new ZipStreamReader
    pub const fn new(reader: R) -> Self {
        Self(StreamingArchive::new(reader))
    }
}

impl<R: Read> ZipStreamReader<R> {
    /// Iterate over the stream and extract all file and their
    /// metadata.
    pub fn visit<V: ZipStreamVisitor>(mut self, visitor: &mut V) -> ZipResult<()> {
        while let Some(mut file) = self.0.next_entry()? {
            visitor.visit_file(&mut file)?;
        }

        while let Some(metadata) = self.0.next_metadata_entry()? {
            visitor.visit_additional_metadata(&metadata)?;
        }

        Ok(())
    }

    /// Extract a Zip archive into a directory, overwriting files if they
    /// already exist. Paths are sanitized with [`ZipFile::enclosed_name`].
    ///
    /// Extraction is not atomic; If an error is encountered, some of the files
    /// may be left on disk.
    pub fn extract<P: AsRef<Path>>(self, directory: P) -> ZipResult<()> {
        struct Extractor<'a>(&'a Path);
        impl ZipStreamVisitor for Extractor<'_> {
            fn visit_file(&mut self, file: &mut StreamingZipEntry<impl Read>) -> ZipResult<()> {
                let filepath = file
                    .enclosed_name()
                    .ok_or(ZipError::InvalidArchive("Invalid file path"))?;

                let outpath = self.0.join(filepath);

                if file.is_dir() {
                    fs::create_dir_all(&outpath)?;
                } else {
                    if let Some(p) = outpath.parent() {
                        fs::create_dir_all(p)?;
                    }
                    let mut outfile = fs::File::create(&outpath)?;
                    io::copy(file, &mut outfile)?;
                }

                Ok(())
            }

            #[allow(unused)]
            fn visit_additional_metadata(
                &mut self,
                metadata: &ZipStreamFileMetadata,
            ) -> ZipResult<()> {
                #[cfg(unix)]
                {
                    let filepath = metadata
                        .enclosed_name()
                        .ok_or(ZipError::InvalidArchive("Invalid file path"))?;

                    let outpath = self.0.join(filepath);

                    use std::os::unix::fs::PermissionsExt;
                    if let Some(mode) = metadata.unix_mode() {
                        fs::set_permissions(outpath, fs::Permissions::from_mode(mode))?;
                    }
                }

                Ok(())
            }
        }

        self.visit(&mut Extractor(directory.as_ref()))
    }
}

/// Visitor for ZipStreamReader
pub trait ZipStreamVisitor {
    ///  * `file` - contains the content of the file and most of the metadata,
    ///    except:
    ///     - `comment`: set to an empty string
    ///     - `data_start`: set to 0
    ///     - `external_attributes`: `unix_mode()`: will return None
    fn visit_file(&mut self, file: &mut StreamingZipEntry<impl Read>) -> ZipResult<()>;

    /// This function is guranteed to be called after all `visit_file`s.
    ///
    ///  * `metadata` - Provides missing metadata in `visit_file`.
    fn visit_additional_metadata(&mut self, metadata: &ZipStreamFileMetadata) -> ZipResult<()>;
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::BTreeSet;

    struct DummyVisitor;
    impl ZipStreamVisitor for DummyVisitor {
        fn visit_file(&mut self, _file: &mut StreamingZipEntry<impl Read>) -> ZipResult<()> {
            Ok(())
        }

        fn visit_additional_metadata(
            &mut self,
            _metadata: &ZipStreamFileMetadata,
        ) -> ZipResult<()> {
            Ok(())
        }
    }

    #[allow(dead_code)]
    #[derive(Default, Debug, Eq, PartialEq)]
    struct CounterVisitor(u64, u64);
    impl ZipStreamVisitor for CounterVisitor {
        fn visit_file(&mut self, _file: &mut StreamingZipEntry<impl Read>) -> ZipResult<()> {
            self.0 += 1;
            Ok(())
        }

        fn visit_additional_metadata(
            &mut self,
            _metadata: &ZipStreamFileMetadata,
        ) -> ZipResult<()> {
            self.1 += 1;
            Ok(())
        }
    }

    #[test]
    fn invalid_offset() {
        ZipStreamReader::new(io::Cursor::new(include_bytes!(
            "../../tests/data/invalid_offset.zip"
        )))
        .visit(&mut DummyVisitor)
        .unwrap_err();
    }

    #[test]
    fn invalid_offset2() {
        ZipStreamReader::new(io::Cursor::new(include_bytes!(
            "../../tests/data/invalid_offset2.zip"
        )))
        .visit(&mut DummyVisitor)
        .unwrap_err();
    }

    #[test]
    fn zip_read_streaming() {
        let reader = ZipStreamReader::new(io::Cursor::new(include_bytes!(
            "../../tests/data/mimetype.zip"
        )));

        #[derive(Default)]
        struct V {
            filenames: BTreeSet<Box<str>>,
        }
        impl ZipStreamVisitor for V {
            fn visit_file(&mut self, file: &mut StreamingZipEntry<impl Read>) -> ZipResult<()> {
                if file.is_file() {
                    self.filenames.insert(file.name().into());
                }

                Ok(())
            }
            fn visit_additional_metadata(
                &mut self,
                metadata: &ZipStreamFileMetadata,
            ) -> ZipResult<()> {
                if metadata.is_file() {
                    assert!(
                        self.filenames.contains(metadata.name()),
                        "{} is missing its file content",
                        metadata.name()
                    );
                }

                Ok(())
            }
        }

        reader.visit(&mut V::default()).unwrap();
    }

    #[test]
    fn file_and_dir_predicates() {
        let reader = ZipStreamReader::new(io::Cursor::new(include_bytes!(
            "../../tests/data/files_and_dirs.zip"
        )));

        #[derive(Default)]
        struct V {
            filenames: BTreeSet<Box<str>>,
        }
        impl ZipStreamVisitor for V {
            fn visit_file(&mut self, file: &mut StreamingZipEntry<impl Read>) -> ZipResult<()> {
                let full_name = file.enclosed_name().unwrap();
                let file_name = full_name.file_name().unwrap().to_str().unwrap();
                assert!(
                    (file_name.starts_with("dir") && file.is_dir())
                        || (file_name.starts_with("file") && file.is_file())
                );

                if file.is_file() {
                    self.filenames.insert(file.name().into());
                }

                Ok(())
            }
            fn visit_additional_metadata(
                &mut self,
                metadata: &ZipStreamFileMetadata,
            ) -> ZipResult<()> {
                if metadata.is_file() {
                    assert!(
                        self.filenames.contains(metadata.name()),
                        "{} is missing its file content",
                        metadata.name()
                    );
                }

                Ok(())
            }
        }

        reader.visit(&mut V::default()).unwrap();
    }

    /// test case to ensure we don't preemptively over allocate based on the
    /// declared number of files in the CDE of an invalid zip when the number of
    /// files declared is more than the alleged offset in the CDE
    #[test]
    fn invalid_cde_number_of_files_allocation_smaller_offset() {
        ZipStreamReader::new(io::Cursor::new(include_bytes!(
            "../../tests/data/invalid_cde_number_of_files_allocation_smaller_offset.zip"
        )))
        .visit(&mut DummyVisitor)
        .unwrap_err();
    }

    /// test case to ensure we don't preemptively over allocate based on the
    /// declared number of files in the CDE of an invalid zip when the number of
    /// files declared is less than the alleged offset in the CDE
    #[test]
    fn invalid_cde_number_of_files_allocation_greater_offset() {
        ZipStreamReader::new(io::Cursor::new(include_bytes!(
            "../../tests/data/invalid_cde_number_of_files_allocation_greater_offset.zip"
        )))
        .visit(&mut DummyVisitor)
        .unwrap_err();
    }
}
