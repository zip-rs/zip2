//! Code related to stream reading

use crate::ZipReadOptions;
use crate::read::parse_extra_field;
use crate::read::readers::{make_crypto_reader, make_reader};
use crate::read::{
    ZipFile, ZipFileData, ZipResult, central_header_to_zip_file_inner, make_symlink,
};
use crate::result::ZipError;
use crate::spec::ZipFlags;
use crate::spec::{FixedSizeBlock, Magic, Pod, ZipCentralEntryBlock, ZipLocalEntryBlock};
use indexmap::IndexMap;
use std::borrow::Cow;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// Stream decoder for zip.
#[derive(Debug)]
pub struct ZipStreamReader<R>(R);

impl<R> ZipStreamReader<R> {
    /// Create a new `ZipStreamReader`
    pub const fn new(reader: R) -> Self {
        Self(reader)
    }
}

impl<R: Read> ZipStreamReader<R> {
    fn parse_central_directory(&mut self) -> ZipResult<ZipStreamFileMetadata> {
        // Give archive_offset and central_header_start dummy value 0, since
        // they are not used in the output.
        let archive_offset = 0;
        let central_header_start = 0;

        // Parse central header
        let block = ZipCentralEntryBlock::parse(&mut self.0)?;
        let (file, file_name_raw) = central_header_to_zip_file_inner(
            &mut self.0,
            archive_offset,
            central_header_start,
            block,
        )?;
        Ok(ZipStreamFileMetadata(file, file_name_raw.into()))
    }

    /// Iterate over the stream and extract all file and their
    /// metadata.
    pub fn visit<V: ZipStreamVisitor>(mut self, visitor: &mut V) -> ZipResult<()> {
        while let Some(mut file) = read_zipfile_from_stream(&mut self.0)? {
            visitor.visit_file(&mut file)?;
        }

        while let Ok(metadata) = self.parse_central_directory() {
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
        struct Extractor(PathBuf, IndexMap<Box<[u8]>, ()>);
        impl ZipStreamVisitor for Extractor {
            fn visit_file<R: Read>(&mut self, file: &mut ZipFile<'_, R>) -> ZipResult<()> {
                self.1.insert(file.name_raw().into(), ());
                let mut outpath = self.0.clone();
                file.safe_prepare_path(&self.0, &mut outpath, None::<&(_, fn(&Path) -> bool)>)?;

                if file.is_symlink() {
                    let mut target = Vec::with_capacity(file.size() as usize);
                    file.read_to_end(&mut target)?;
                    make_symlink(&outpath, &target, &self.1)?;
                    return Ok(());
                }

                if file.is_dir() {
                    fs::create_dir_all(&outpath)?;
                } else {
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
                    use super::ZipError;
                    use std::os::unix::fs::PermissionsExt;
                    let filepath = metadata
                        .enclosed_name()
                        .ok_or(crate::result::invalid!("Invalid file path"))?;

                    let outpath = self.0.join(filepath);

                    if let Some(mode) = metadata.unix_mode() {
                        fs::set_permissions(outpath, fs::Permissions::from_mode(mode))?;
                    }
                }

                Ok(())
            }
        }
        use std::fs;
        fs::create_dir_all(&directory)?;
        let directory = directory.as_ref().canonicalize()?;

        self.visit(&mut Extractor(directory, IndexMap::new()))
    }
}

/// Visitor for `ZipStreamReader`
pub trait ZipStreamVisitor {
    ///  * `file` - contains the content of the file and most of the metadata,
    ///    except:
    ///     - `comment`: set to an empty string
    ///     - `data_start`: set to 0
    ///     - `external_attributes`: `unix_mode()`: will return None
    fn visit_file<R: Read>(&mut self, file: &mut ZipFile<'_, R>) -> ZipResult<()>;

    /// This function is guaranteed to be called after all `visit_file`s.
    ///
    ///  * `metadata` - Provides missing metadata in `visit_file`.
    fn visit_additional_metadata(&mut self, metadata: &ZipStreamFileMetadata) -> ZipResult<()>;
}

/// Additional metadata for the file.
#[derive(Debug)]
pub struct ZipStreamFileMetadata(ZipFileData, Box<[u8]>);

impl ZipStreamFileMetadata {
    /// Get the name of the file
    ///
    /// # Warnings
    ///
    /// It is dangerous to use this name directly when extracting an archive.
    /// It may contain an absolute path (`/etc/shadow`), or break out of the
    /// current directory (`../runtime`). Carelessly writing to these paths
    /// allows an attacker to craft a ZIP archive that will overwrite critical
    /// files.
    ///
    /// You can use the [`ZipFile::enclosed_name`] method to validate the name
    /// as a safe path.
    pub fn name(&self) -> ZipResult<Cow<'_, str>> {
        self.0.name(&self.1)
    }

    /// Get the name of the file, in the raw (internal) byte representation.
    ///
    /// The encoding of this data is currently undefined.
    pub fn name_raw(&self) -> &[u8] {
        &self.1
    }

    /// Rewrite the path, ignoring any path components with special meaning.
    ///
    /// - Absolute paths are made relative
    /// - [`std::path::Component::ParentDir`]s are ignored
    /// - Truncates the filename at a NULL byte
    ///
    /// This is appropriate if you need to be able to extract *something* from
    /// any archive, but will easily misrepresent trivial paths like
    /// `foo/../bar` as `foo/bar` (instead of `bar`). Because of this,
    /// [`ZipFile::enclosed_name`] is the better option in most scenarios.
    pub fn mangled_name(&self) -> ZipResult<PathBuf> {
        let name = self.name()?;
        let sanitized = self.0.file_name_sanitized(&name);
        Ok(sanitized)
    }

    /// Ensure the file path is safe to use as a [`Path`].
    ///
    /// - It can't contain NULL bytes
    /// - It can't resolve to a path outside the current directory
    ///   > `foo/../bar` is fine, `foo/../../bar` is not.
    /// - It can't be an absolute path
    ///
    /// This will read well-formed ZIP files correctly, and is resistant
    /// to path-based exploits. It is recommended over
    /// [`ZipFile::mangled_name`].
    pub fn enclosed_name(&self) -> Option<PathBuf> {
        let Ok(name) = self.name() else {
            return None;
        };
        let enclosed = self.0.enclosed_name(&name)?;
        Some(enclosed)
    }

    /// Returns whether the file is actually a directory
    pub fn is_dir(&self) -> bool {
        self.0.is_dir(&self.1)
    }

    /// Returns whether the file is a regular file
    pub fn is_file(&self) -> bool {
        !self.is_dir()
    }

    /// Get the comment of the file
    pub fn comment(&self) -> &str {
        &self.0.file_comment
    }

    /// Get unix mode for the file
    pub const fn unix_mode(&self) -> Option<u32> {
        self.0.unix_mode()
    }
}

/// Read `ZipFile` structures from a non-seekable reader.
///
/// This is an alternative method to read a zip file. If possible, use the `ZipArchive` functions
/// as some information will be missing when reading this manner.
///
/// Reads a file header from the start of the stream. Will return `Ok(Some(..))` if a file is
/// present at the start of the stream. Returns `Ok(None)` if the start of the central directory
/// is encountered. No more files should be read after this.
///
/// The Drop implementation of `ZipFile` ensures that the reader will be correctly positioned after
/// the structure is done.
///
/// Missing fields are:
/// * `comment`: set to an empty string
/// * `data_start`: set to 0
/// * `external_attributes`: `unix_mode()`: will return None
pub fn read_zipfile_from_stream<R: Read>(reader: &mut R) -> ZipResult<Option<ZipFile<'_, R>>> {
    read_zipfile_from_stream_with_options(reader, ZipReadOptions::default())
}

/// Read `ZipFile` from a non-seekable reader like [`read_zipfile_from_stream`] does, but assume the
/// given compressed size and don't read any further ahead than that.
pub fn read_zipfile_from_stream_with_compressed_size<'a, R: io::Read>(
    reader: &'a mut R,
    compressed_size: u64,
) -> ZipResult<Option<ZipFile<'a, R>>> {
    let options = ZipReadOptions::default().override_compressed_size(compressed_size);
    read_zipfile_from_stream_with_options(reader, options)
}

/// Same as `read_zipfile_from_stream` but with `ZipReadOptions`
/// Since LZMA decoding requires the uncompressed length, you will need to override it
pub fn read_zipfile_from_stream_with_options<'a, R: io::Read>(
    reader: &'a mut R,
    mut options: ZipReadOptions<'a>,
) -> ZipResult<Option<ZipFile<'a, R>>> {
    // We can't use the typical [`ZipLocalEntryBlock::parse`] method, as we follow separate code paths depending on the
    // "magic" value (since the magic value will be from the central directory header if we've
    // finished iterating over all the actual files).
    let mut magic_buf = [0; size_of::<u32>()];
    reader.read_exact(&mut magic_buf)?;

    match Magic::from_le_bytes(magic_buf) {
        Magic::LOCAL_FILE_HEADER_SIGNATURE => (),
        Magic::CENTRAL_DIRECTORY_HEADER_SIGNATURE => return Ok(None),
        _ => return Err(ZipLocalEntryBlock::WRONG_MAGIC_ERROR),
    }

    let mut block = ZipLocalEntryBlock::zeroed();
    reader.read_exact(block.as_bytes_mut())?;

    let block = block.from_le();

    let (mut data, mut file_name_raw) = ZipFileData::from_local_block(block, reader)?;
    let using_data_descriptor: bool = ZipFlags::matching(data.flags, ZipFlags::UsingDataDescriptor);
    if using_data_descriptor {
        if let Some(comp_size) = options.force_compressed_size {
            data.compressed_size = comp_size;
        } else {
            return Err(ZipError::UnsupportedArchive(
                "The file length is not available in the local header",
            ));
        }
    }
    if let Some(uncomp_size) = options.force_uncompressed_size {
        data.uncompressed_size = uncomp_size;
    }
    if let Some(crc) = options.force_crc {
        data.crc32 = crc;
    }

    match parse_extra_field(&mut data, &mut file_name_raw) {
        Ok(..) | Err(ZipError::Io(..)) => {}
        Err(e) => return Err(e),
    }

    if options.ignore_encryption_flag {
        // Always use no password when we're ignoring the encryption flag.
        options.password = None;
    } else {
        // Require and use the password only if the file is encrypted.
        match (options.password, data.is_encrypted()) {
            (None, true) => {
                return Err(ZipError::UnsupportedArchive(ZipError::PASSWORD_REQUIRED));
            }
            // Password supplied, but none needed! Discard.
            (Some(_), false) => options.password = None,
            _ => {}
        }
    }

    let limit_reader = reader.take(data.compressed_size);
    let crypto_reader = make_crypto_reader(&data, limit_reader, options.password)?;
    let ZipFileData {
        compression_method,
        uncompressed_size,
        #[cfg(feature = "legacy-zip")]
        flags,
        ..
    } = data;
    let checksum = if options.ignore_crc {
        None
    } else {
        Some(data.crc32)
    };

    let vendor_version = data.aes_mode.map(|aes| aes.1);
    Ok(Some(ZipFile {
        file_name_raw: Cow::Owned(file_name_raw),
        data: Cow::Owned(data),
        reader: make_reader(
            compression_method,
            uncompressed_size,
            checksum,
            vendor_version,
            crypto_reader,
            #[cfg(feature = "legacy-zip")]
            flags,
        )?,
    }))
}

#[cfg(test)]
mod tests {

    use crate::read::ZipFile;
    use crate::read::stream::{ZipStreamFileMetadata, ZipStreamReader, ZipStreamVisitor};
    use crate::result::ZipResult;
    use std::collections::BTreeSet;
    use std::io::{Cursor, Read};

    struct DummyVisitor;
    impl ZipStreamVisitor for DummyVisitor {
        fn visit_file<R: Read>(&mut self, _file: &mut ZipFile<'_, R>) -> ZipResult<()> {
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
        fn visit_file<R: Read>(&mut self, _file: &mut ZipFile<'_, R>) -> ZipResult<()> {
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
        ZipStreamReader::new(Cursor::new(include_bytes!(
            "../../tests/data/invalid_offset.zip"
        )))
        .visit(&mut DummyVisitor)
        .unwrap_err();
    }

    #[test]
    fn invalid_offset2() {
        ZipStreamReader::new(Cursor::new(include_bytes!(
            "../../tests/data/invalid_offset2.zip"
        )))
        .visit(&mut DummyVisitor)
        .unwrap_err();
    }

    #[test]
    fn zip_read_streaming_visitor() {
        let reader =
            ZipStreamReader::new(Cursor::new(include_bytes!("../../tests/data/mimetype.zip")));

        #[derive(Default)]
        struct V {
            filenames: BTreeSet<Box<str>>,
        }
        impl ZipStreamVisitor for V {
            fn visit_file<R: Read>(&mut self, file: &mut ZipFile<'_, R>) -> ZipResult<()> {
                if file.is_file() {
                    let file_name = file.name().unwrap();
                    self.filenames.insert(file_name.into());
                }

                Ok(())
            }
            fn visit_additional_metadata(
                &mut self,
                zip_file_metadata: &ZipStreamFileMetadata,
            ) -> ZipResult<()> {
                if zip_file_metadata.is_file() {
                    let file_name = zip_file_metadata.name().unwrap();
                    assert!(
                        self.filenames.contains::<str>(file_name.as_ref()),
                        "{} is missing its file content",
                        file_name
                    );
                }

                Ok(())
            }
        }

        reader.visit(&mut V::default()).unwrap();
    }

    #[test]
    fn file_and_dir_predicates() {
        let reader = ZipStreamReader::new(Cursor::new(include_bytes!(
            "../../tests/data/files_and_dirs.zip"
        )));

        #[derive(Default)]
        struct V {
            filenames: BTreeSet<Box<str>>,
        }
        impl ZipStreamVisitor for V {
            fn visit_file<R: Read>(&mut self, file: &mut ZipFile<'_, R>) -> ZipResult<()> {
                let full_name = file.enclosed_name().unwrap();
                let file_name = full_name.file_name().unwrap().to_str().unwrap();
                assert!(
                    (file_name.starts_with("dir") && file.is_dir())
                        || (file_name.starts_with("file") && file.is_file())
                );

                if file.is_file() {
                    let file_name = file.name().unwrap();
                    self.filenames.insert(file_name.into());
                }

                Ok(())
            }
            fn visit_additional_metadata(
                &mut self,
                zip_file_metadata: &ZipStreamFileMetadata,
            ) -> ZipResult<()> {
                if zip_file_metadata.is_file() {
                    let file_name = zip_file_metadata.name().unwrap();
                    assert!(
                        self.filenames.contains::<str>(file_name.as_ref()),
                        "{} is missing its file content",
                        file_name
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
        ZipStreamReader::new(Cursor::new(include_bytes!(
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
        ZipStreamReader::new(Cursor::new(include_bytes!(
            "../../tests/data/invalid_cde_number_of_files_allocation_greater_offset.zip"
        )))
        .visit(&mut DummyVisitor)
        .unwrap_err();
    }

    /// Symlinks being extracted shouldn't be followed out of the destination directory.
    /// Only on little endian because we cannot use fs with miri CI
    #[cfg(all(target_endian = "little", not(miri)))]
    #[test]
    fn test_cannot_symlink_outside_destination() -> ZipResult<()> {
        use crate::ZipWriter;
        use crate::write::SimpleFileOptions;
        use std::fs::create_dir;
        use tempfile::TempDir;

        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.add_symlink("symlink/", "../dest-sibling/", SimpleFileOptions::default())?;
        writer.start_file("symlink/dest-file", SimpleFileOptions::default())?;
        let reader = ZipStreamReader::new(writer.finish()?);
        let dest_parent = TempDir::with_prefix("stream__cannot_symlink_outside_destination")?;
        let dest_sibling = dest_parent.path().join("dest-sibling");
        create_dir(&dest_sibling)?;
        let dest = dest_parent.path().join("dest");
        create_dir(&dest)?;
        assert!(reader.extract(dest).is_err());
        assert!(!dest_sibling.join("dest-file").exists());
        Ok(())
    }

    /// Only on little endian because we cannot use fs with miri CI
    #[cfg(all(target_endian = "little", not(miri)))]
    #[test]
    fn test_can_create_destination() -> ZipResult<()> {
        use tempfile::TempDir;

        let v = include_bytes!("../../tests/data/mimetype.zip");
        let reader = ZipStreamReader::new(v.as_ref());
        let dest = TempDir::with_prefix("stream_test_can_create_destination").unwrap();
        reader.extract(&dest)?;
        assert!(dest.path().join("mimetype").exists());
        Ok(())
    }

    #[test]
    fn zip_read_streaming() {
        use super::read_zipfile_from_stream;

        let mut reader = Cursor::new(include_bytes!("../../tests/data/mimetype.zip"));
        loop {
            if read_zipfile_from_stream(&mut reader).unwrap().is_none() {
                break;
            }
        }
    }

    #[test]
    #[cfg(feature = "deflate")]
    fn zip_read_streaming_compressed() {
        use super::read_zipfile_from_stream_with_compressed_size;
        use crate::ZipWriter;
        use crate::write::SimpleFileOptions;
        use std::io::Write;

        let compression_method = crate::CompressionMethod::Deflated;
        let options = SimpleFileOptions::default()
            .compression_method(compression_method)
            .unix_permissions(0o755);

        let mut bytes = Vec::new();
        let mut writer = ZipWriter::new(std::io::Cursor::new(&mut bytes));
        writer.start_file("file.txt", options).unwrap();
        write!(&mut writer, "{}", "test-".repeat(100)).unwrap();
        writer.finish().unwrap();

        let compressed_size = u32::from_le_bytes(bytes[18..22].try_into().unwrap());
        let uncompressed_size = u32::from_le_bytes(bytes[22..26].try_into().unwrap());

        assert_eq!(compressed_size, 14);
        assert_eq!(uncompressed_size as usize, "test-".len() * 100);

        let mut reader = Cursor::new(bytes);
        loop {
            if read_zipfile_from_stream_with_compressed_size(
                &mut reader,
                u64::from(compressed_size),
            )
            .unwrap()
            .is_none()
            {
                break;
            }
        }
    }

    #[test]
    #[cfg(feature = "aes-crypto")]
    fn zip_read_streaming_compressed_and_aes() {
        use super::read_zipfile_from_stream_with_options;
        use crate::ZipReadOptions;

        let bytes = include_bytes!("../../tests/data/aes_archive.zip");
        let compressed_size = 46;

        let mut reader = Cursor::new(bytes);
        const PASSWORD: &[u8] = b"helloworld";
        let options = ZipReadOptions::new()
            .password(Some(PASSWORD))
            .override_compressed_size(compressed_size);

        // we simulate the fact that we need the compressed size like a streamed zip
        let result = read_zipfile_from_stream_with_options(&mut reader, options);
        let optional_file = result.unwrap();
        let mut file = optional_file.unwrap();

        let file_name = file.name().unwrap();
        assert_eq!(file_name, "secret_data_128");

        const SECRET_CONTENT: &str = "Lorem ipsum dolor sit amet";
        let mut decrypted_content = String::new();
        file.read_to_string(&mut decrypted_content)
            .expect("couldn't read encrypted file");
        assert_eq!(SECRET_CONTENT, decrypted_content);
    }

    #[test]
    #[cfg(feature = "aes-crypto")]
    fn zip_read_streaming_compressed_and_aes_without_size() {
        use super::read_zipfile_from_stream_with_options;
        use crate::ZipReadOptions;

        let bytes = include_bytes!("../../tests/data/aes_archive.zip");

        let mut reader = Cursor::new(bytes);
        const PASSWORD: &[u8] = b"helloworld";
        let options = ZipReadOptions::new().password(Some(PASSWORD));

        // the zip already has the compressed size (it's not a streamed zip)
        let result = read_zipfile_from_stream_with_options(&mut reader, options);
        let optional_file = result.unwrap();
        let mut file = optional_file.unwrap();

        let file_name = file.name().unwrap();
        assert_eq!(file_name, "secret_data_128");

        const SECRET_CONTENT: &str = "Lorem ipsum dolor sit amet";
        let mut decrypted_content = String::new();
        file.read_to_string(&mut decrypted_content)
            .expect("couldn't read encrypted file");
        assert_eq!(SECRET_CONTENT, decrypted_content);
    }

    #[test]
    fn zip_read_streaming_zipwriter() {
        use crate::CompressionMethod;
        use crate::ZipReadOptions;
        use crate::ZipWriter;
        use crate::read::read_zipfile_from_stream;
        use crate::read::read_zipfile_from_stream_with_compressed_size;
        use crate::read::read_zipfile_from_stream_with_options;
        use crate::write::SimpleFileOptions;
        use std::io::Write;

        let mut buffer = Vec::new();
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let mut archive = ZipWriter::new_stream(Cursor::new(&mut buffer));
        archive.start_file("name", options).unwrap();
        archive.write_all(b"test").unwrap();
        let compressed_size = b"test".len() as u64;
        archive.finish().unwrap();
        {
            // reading will fail because it's a streamed zipfile and we don't know the size
            let mut reader = Cursor::new(buffer.clone());
            let result = read_zipfile_from_stream(&mut reader);
            assert!(result.is_err());
        }
        {
            // reading the file will fail because of the invalid checksum
            let mut reader = Cursor::new(buffer.clone());
            let result =
                read_zipfile_from_stream_with_compressed_size(&mut reader, compressed_size);
            let optional_file = result.unwrap();
            let mut file = optional_file.unwrap();
            let file_name = file.name().unwrap();
            assert_eq!(file_name, "name");
            // test reading
            let mut content = Vec::new();
            let read_result = file.read_to_end(&mut content);
            assert!(read_result.is_err()); // invalid checksum

            let error_str = read_result.unwrap_err().to_string();
            // eprintln!("{error_str}");
            assert!(error_str.contains("Invalid checksum"));
        }
        {
            // reading the file will work because we override the checksum
            let mut reader = Cursor::new(buffer.clone());
            let read_options = ZipReadOptions::default()
                .override_compressed_size(compressed_size)
                .override_crc(3632233996);
            let result = read_zipfile_from_stream_with_options(&mut reader, read_options);
            let optional_file = result.unwrap();
            let mut file = optional_file.unwrap();
            let file_name = file.name().unwrap();
            assert_eq!(file_name, "name");
            // test reading
            let mut content = Vec::new();
            file.read_to_end(&mut content).unwrap();
            assert_eq!(content, b"test");
        }
        {
            // reading the file will work because we skip checksum
            let mut reader = Cursor::new(buffer.clone());
            let read_options = ZipReadOptions::default()
                .override_compressed_size(compressed_size)
                .ignore_crc32(true);
            let result = read_zipfile_from_stream_with_options(&mut reader, read_options);
            let optional_file = result.unwrap();
            let mut file = optional_file.unwrap();
            let file_name = file.name().unwrap();
            assert_eq!(file_name, "name");
            // test reading
            let mut content = Vec::new();
            let read_result = file.read_to_end(&mut content);
            assert!(read_result.is_ok());
            assert_eq!(content, b"test");
        }
    }
}
