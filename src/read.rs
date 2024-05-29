//! Types for reading ZIP archives

/// Module for code with synchronous logic
#[cfg(feature = "sync")]
pub mod sync;

/// Module for code with asynchronous logic
#[cfg(feature = "tokio")]
pub mod tokio;

#[cfg(feature = "aes-crypto")]
use crate::{aes::AesReaderValid, types::AesVendorVersion};
use crate::{crc32::Crc32Reader, types::ZipFileData, zipcrypto::ZipCryptoReaderValid};
use indexmap::IndexMap;
use std::borrow::Cow;
use std::io::{self, prelude::*};

#[cfg(any(
    feature = "deflate",
    feature = "deflate-zlib",
    feature = "deflate-zlib-ng"
))]
use flate2::read::DeflateDecoder;

#[cfg(feature = "deflate64")]
use deflate64::Deflate64Decoder;

#[cfg(feature = "bzip2")]
use bzip2::read::BzDecoder;

#[cfg(feature = "zstd")]
use zstd::stream::read::Decoder as ZstdDecoder;

/// Provides high level API for reading from a stream.
pub(crate) mod stream;

#[cfg(feature = "lzma")]
pub(crate) mod lzma;

// Put the struct declaration in a private module to convince rustdoc to display ZipArchive nicely
pub(crate) mod zip_archive {
    use std::sync::Arc;

    /// Extract immutable data from `ZipArchive` to make it cheap to clone
    #[derive(Debug)]
    pub(crate) struct Shared {
        pub(crate) files: super::IndexMap<Box<str>, super::ZipFileData>,
        pub(super) offset: u64,
        pub(super) dir_start: u64,
    }

    /// ZIP archive reader
    ///
    /// At the moment, this type is cheap to clone if this is the case for the
    /// reader it uses. However, this is not guaranteed by this crate and it may
    /// change in the future.
    ///
    /// ```no_run
    /// use std::io::prelude::*;
    /// fn list_zip_contents(reader: impl Read + Seek) -> zip::result::ZipResult<()> {
    ///     let mut zip = zip::ZipArchive::new(reader)?;
    ///
    ///     for i in 0..zip.len() {
    ///         let mut file = zip.by_index(i)?;
    ///         println!("Filename: {}", file.name());
    ///         std::io::copy(&mut file, &mut std::io::stdout())?;
    ///     }
    ///
    ///     Ok(())
    /// }
    /// ```
    #[derive(Clone, Debug)]
    pub struct ZipArchive<R> {
        pub(super) reader: R,
        pub(super) shared: Arc<Shared>,
        pub(super) comment: Arc<[u8]>,
    }
}

#[cfg(feature = "lzma")]
use crate::read::lzma::LzmaDecoder;
pub use zip_archive::ZipArchive;

#[allow(clippy::large_enum_variant)]
pub(crate) enum CryptoReader<'a> {
    Plaintext(io::Take<&'a mut dyn Read>),
    ZipCrypto(ZipCryptoReaderValid<io::Take<&'a mut dyn Read>>),
    #[cfg(feature = "aes-crypto")]
    Aes {
        reader: AesReaderValid<io::Take<&'a mut dyn Read>>,
        vendor_version: AesVendorVersion,
    },
}

pub(crate) enum ZipFileReader<'a> {
    NoReader,
    Raw(io::Take<&'a mut dyn Read>),
    Stored(Crc32Reader<CryptoReader<'a>>),
    #[cfg(feature = "_deflate-any")]
    Deflated(Crc32Reader<DeflateDecoder<CryptoReader<'a>>>),
    #[cfg(feature = "deflate64")]
    Deflate64(Crc32Reader<Deflate64Decoder<io::BufReader<CryptoReader<'a>>>>),
    #[cfg(feature = "bzip2")]
    Bzip2(Crc32Reader<BzDecoder<CryptoReader<'a>>>),
    #[cfg(feature = "zstd")]
    Zstd(Crc32Reader<ZstdDecoder<'a, io::BufReader<CryptoReader<'a>>>>),
    #[cfg(feature = "lzma")]
    Lzma(Crc32Reader<Box<LzmaDecoder<CryptoReader<'a>>>>),
}

/// A struct for reading a zip file
pub struct ZipFile<'a> {
    pub(crate) data: Cow<'a, ZipFileData>,
    pub(crate) crypto_reader: Option<CryptoReader<'a>>,
    pub(crate) reader: ZipFileReader<'a>,
}
pub(crate) struct CentralDirectoryInfo {
    pub(crate) archive_offset: u64,
    pub(crate) directory_start: u64,
    pub(crate) number_of_files: usize,
    pub(crate) disk_number: u32,
    pub(crate) disk_with_central_directory: u32,
}

#[cfg(test)]
mod test {
    use crate::ZipArchive;
    use std::io::Cursor;

    #[test]
    fn invalid_offset() {
        use super::ZipArchive;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/invalid_offset.zip"));
        let reader = ZipArchive::new(Cursor::new(v));
        assert!(reader.is_err());
    }

    #[test]
    fn invalid_offset2() {
        use super::ZipArchive;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/invalid_offset2.zip"));
        let reader = ZipArchive::new(Cursor::new(v));
        assert!(reader.is_err());
    }

    #[test]
    fn zip64_with_leading_junk() {
        use super::ZipArchive;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/zip64_demo.zip"));
        let reader = ZipArchive::new(Cursor::new(v)).unwrap();
        assert_eq!(reader.len(), 1);
    }

    #[test]
    fn zip_contents() {
        use super::ZipArchive;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/mimetype.zip"));
        let mut reader = ZipArchive::new(Cursor::new(v)).unwrap();
        assert_eq!(reader.comment(), b"");
        assert_eq!(reader.by_index(0).unwrap().central_header_start(), 77);
    }

    #[test]
    fn zip_read_streaming() {
        use super::sync::read_zipfile_from_stream;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/mimetype.zip"));
        let mut reader = Cursor::new(v);
        loop {
            if read_zipfile_from_stream(&mut reader).unwrap().is_none() {
                break;
            }
        }
    }

    #[test]
    fn zip_clone() {
        use super::ZipArchive;
        use std::io::Read;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/mimetype.zip"));
        let mut reader1 = ZipArchive::new(Cursor::new(v)).unwrap();
        let mut reader2 = reader1.clone();

        let mut file1 = reader1.by_index(0).unwrap();
        let mut file2 = reader2.by_index(0).unwrap();

        let t = file1.last_modified();
        assert_eq!(
            (
                t.year(),
                t.month(),
                t.day(),
                t.hour(),
                t.minute(),
                t.second()
            ),
            (1980, 1, 1, 0, 0, 0)
        );

        let mut buf1 = [0; 5];
        let mut buf2 = [0; 5];
        let mut buf3 = [0; 5];
        let mut buf4 = [0; 5];

        file1.read_exact(&mut buf1).unwrap();
        file2.read_exact(&mut buf2).unwrap();
        file1.read_exact(&mut buf3).unwrap();
        file2.read_exact(&mut buf4).unwrap();

        assert_eq!(buf1, buf2);
        assert_eq!(buf3, buf4);
        assert_ne!(buf1, buf3);
    }

    #[test]
    fn file_and_dir_predicates() {
        use super::ZipArchive;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/files_and_dirs.zip"));
        let mut zip = ZipArchive::new(Cursor::new(v)).unwrap();

        for i in 0..zip.len() {
            let zip_file = zip.by_index(i).unwrap();
            let full_name = zip_file.enclosed_name().unwrap();
            let file_name = full_name.file_name().unwrap().to_str().unwrap();
            assert!(
                (file_name.starts_with("dir") && zip_file.is_dir())
                    || (file_name.starts_with("file") && zip_file.is_file())
            );
        }
    }

    #[test]
    fn zip64_magic_in_filenames() {
        let files = vec![
            include_bytes!("../tests/data/zip64_magic_in_filename_1.zip").to_vec(),
            include_bytes!("../tests/data/zip64_magic_in_filename_2.zip").to_vec(),
            include_bytes!("../tests/data/zip64_magic_in_filename_3.zip").to_vec(),
            include_bytes!("../tests/data/zip64_magic_in_filename_4.zip").to_vec(),
            include_bytes!("../tests/data/zip64_magic_in_filename_5.zip").to_vec(),
        ];
        // Although we don't allow adding files whose names contain the ZIP64 CDB-end or
        // CDB-end-locator signatures, we still read them when they aren't genuinely ambiguous.
        for file in files {
            ZipArchive::new(Cursor::new(file)).unwrap();
        }
    }

    /// test case to ensure we don't preemptively over allocate based on the
    /// declared number of files in the CDE of an invalid zip when the number of
    /// files declared is more than the alleged offset in the CDE
    #[test]
    fn invalid_cde_number_of_files_allocation_smaller_offset() {
        use super::ZipArchive;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!(
            "../tests/data/invalid_cde_number_of_files_allocation_smaller_offset.zip"
        ));
        let reader = ZipArchive::new(Cursor::new(v));
        assert!(reader.is_err() || reader.unwrap().is_empty());
    }

    /// test case to ensure we don't preemptively over allocate based on the
    /// declared number of files in the CDE of an invalid zip when the number of
    /// files declared is less than the alleged offset in the CDE
    #[test]
    fn invalid_cde_number_of_files_allocation_greater_offset() {
        use super::ZipArchive;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!(
            "../tests/data/invalid_cde_number_of_files_allocation_greater_offset.zip"
        ));
        let reader = ZipArchive::new(Cursor::new(v));
        assert!(reader.is_err());
    }

    #[cfg(feature = "deflate64")]
    #[test]
    fn deflate64_index_out_of_bounds() -> std::io::Result<()> {
        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!(
            "../tests/data/raw_deflate64_index_out_of_bounds.zip"
        ));
        let mut reader = ZipArchive::new(Cursor::new(v))?;
        std::io::copy(&mut reader.by_index(0)?, &mut std::io::sink()).expect_err("Invalid file");
        Ok(())
    }

    #[cfg(feature = "deflate64")]
    #[test]
    fn deflate64_not_enough_space() {
        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/deflate64_issue_25.zip"));
        ZipArchive::new(Cursor::new(v)).expect_err("Invalid file");
    }

    #[cfg(feature = "_deflate-any")]
    #[test]
    fn test_read_with_data_descriptor() {
        use std::io::Read;

        let mut v = Vec::new();
        v.extend_from_slice(include_bytes!("../tests/data/data_descriptor.zip"));
        let mut reader = ZipArchive::new(Cursor::new(v)).unwrap();
        let mut decompressed = [0u8; 16];
        let mut file = reader.by_index(0).unwrap();
        assert_eq!(file.read(&mut decompressed).unwrap(), 12);
    }
}
