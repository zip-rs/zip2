#![allow(unexpected_cfgs)] // Needed for cfg(fuzzing) on nightly as of 2024-05-06

#[cfg(fuzzing)]
use afl::fuzz;
use sha3::{Digest, Sha3_512};
use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::io::{Read, Seek, SeekFrom};
#[cfg(fuzzing)]
use tikv_jemallocator::Jemalloc;
use zip::ZipReadOptions;
use zip::read::{ZipFile, ZipFileEntry, read_zipfile_from_stream};
use zip::result::ZipError;
use zip::unstable::stream::{ZipStreamReader, ZipStreamVisitor};

const MAX_BYTES_TO_READ: u64 = 1 << 24;

#[cfg(fuzzing)]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

fn decompress_all(reader: impl Read + Seek) -> Result<(), Box<dyn std::error::Error>> {
    let mut zip = zip::ZipArchive::new(reader)?;

    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?.take(MAX_BYTES_TO_READ);
        std::io::copy(&mut file, &mut std::io::sink())?;
    }
    let mut reader = zip.into_inner();
    reader.seek(SeekFrom::Start(0))?;
    while let Ok(Some(mut file)) = read_zipfile_from_stream(&mut reader) {
        std::io::copy(&mut file, &mut std::io::sink())?;
    }
    Ok(())
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct FileMetadata {
    name: Vec<u8>,
    comment: String,
    size: u64,
    compressed_size: u64,
    crc32: u32,
    compression: String,
    encrypted: bool,
    version_made_by: (u8, u8),
    version_needed: u16,
    last_modified: Option<String>,
    system: String,
    is_dir: bool,
    is_symlink: bool,
    is_read_only: bool,
    unix_mode: Option<u32>,
    external_attributes: u32,
    flags: String,
}

macro_rules! file_metadata {
    ($file:expr) => {
        FileMetadata {
            name: $file.name_raw().to_vec(),
            comment: $file.comment().to_owned(),
            size: $file.size(),
            compressed_size: $file.compressed_size(),
            crc32: $file.crc32(),
            compression: format!("{:?}", $file.compression()),
            encrypted: $file.encrypted(),
            version_made_by: $file.version_made_by(),
            version_needed: $file.version_needed(),
            last_modified: $file.last_modified().map(|time| format!("{time:?}")),
            system: format!("{:?}", $file.system()),
            is_dir: $file.is_dir(),
            is_symlink: $file.is_symlink(),
            is_read_only: $file.is_read_only(),
            unix_mode: $file.unix_mode(),
            external_attributes: $file.external_attributes(),
            flags: format!("{:?}", $file.flags()),
        }
    };
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct FileChecksum {
    name: Vec<u8>,
    checksum: [u8; 64],
}

#[derive(Debug)]
struct ReaderMismatch(Cow<'static, str>);

impl From<ZipError> for ReaderMismatch {
    fn from(value: ZipError) -> Self {
        Self(Cow::Owned(value.to_string()))
    }
}

impl fmt::Display for ReaderMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "ZipArchive and ZipStreamReader disagree on {}",
            self.0
        )
    }
}

impl Error for ReaderMismatch {}

fn checksum(reader: &mut impl Read, size: u64) -> std::io::Result<[u8; 64]> {
    if size > MAX_BYTES_TO_READ {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "file exceeds the fuzz target read limit",
        ));
    }

    let mut hasher = Sha3_512::new();
    let mut buffer = [0; 8192];
    let mut remaining = size;
    while remaining > 0 {
        let length = buffer.len().min(remaining as usize);
        let count = reader.read(&mut buffer[..length])?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        remaining -= count as u64;
    }
    if remaining == 0 {
        let mut extra_byte = [0];
        if reader.read(&mut extra_byte)? != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "file exceeds the fuzz target read limit",
            ));
        }
    }
    Ok(hasher.finalize().into())
}

#[derive(Default)]
struct StreamResults {
    metadata: Vec<FileMetadata>,
    checksums: Vec<FileChecksum>,
}

impl ZipStreamVisitor for StreamResults {
    fn visit_file<R: Read>(&mut self, file: &mut ZipFile<'_, R>) -> zip::result::ZipResult<()> {
        let checksum = checksum(file, file.size())?;
        self.checksums.push(FileChecksum {
            name: file.name_raw().to_vec(),
            checksum,
        });
        Ok(())
    }

    fn visit_additional_metadata(
        &mut self,
        metadata: &ZipFileEntry<'_>,
    ) -> zip::result::ZipResult<()> {
        self.metadata.push(file_metadata!(metadata));
        Ok(())
    }
}

fn compare_readers(data: &[u8]) -> Result<(), ReaderMismatch> {
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(data)) else {
        return Ok(());
    };

    if archive.validate_header_consistency().is_err() {
        let mut data = std::io::Cursor::new(data);
        while let Ok(Some(mut file)) = read_zipfile_from_stream(&mut data) {
            std::io::copy(&mut file, &mut std::io::sink()).ok();
        }
        return Ok(());
    }
    let mut archive_metadata = Vec::new();
    let mut archive_checksums = Vec::new();
    let mut archive_checksums_complete = true;
    for index in 0..archive.len() {
        let mut file = archive.by_index_with_options(
            index,
            ZipReadOptions::new().enforce_header_consistency(true),
        )?;
        let metadata = file_metadata!(file);
        let size = file.size();
        if size > MAX_BYTES_TO_READ {
            archive_checksums_complete = false;
            archive_metadata.push(metadata);
            continue;
        }
        archive_metadata.push(metadata);
        match checksum(&mut file, size) {
            Ok(checksum) => archive_checksums.push(FileChecksum {
                name: file.name_raw().to_vec(),
                checksum,
            }),
            Err(_) => archive_checksums_complete = false,
        }
    }

    let mut stream_results = StreamResults::default();
    if ZipStreamReader::new(std::io::Cursor::new(data))
        .enforce_header_consistency(true)
        .visit(&mut stream_results)
        .is_err()
    {
        return Ok(());
    }

    archive_metadata.sort();
    stream_results.metadata.sort();
    if archive_metadata != stream_results.metadata {
        return Err(ReaderMismatch("file metadata".into()));
    }
    if !archive_checksums_complete {
        return Ok(());
    }
    archive_checksums.sort();
    stream_results.checksums.sort();
    if archive_checksums != stream_results.checksums {
        return Err(ReaderMismatch("SHA3-512 checksums".into()));
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    #[cfg(fuzzing)]
    fuzz!(|data: &[u8]| {
        let reader = std::io::Cursor::new(data);
        let _ = decompress_all(reader);
        if let Err(error) = compare_readers(data) {
            panic!("{error}");
        }
    });

    #[cfg(not(fuzzing))]
    {
        let mut v = Vec::new();
        std::io::stdin().read_to_end(&mut v).unwrap();
        let reader = std::io::Cursor::new(&v);
        // Invalid archives are expected; only panics/hangs are bugs.
        let _ = decompress_all(reader);
        // A mismatch between successful, consistency-enforcing readers is a bug.
        compare_readers(&v)?;
    }
    Ok(())
}
