//! ZIP reader APIs backed by the common format and metadata types.

pub use zip_core_lib::read::*;
pub mod config;
pub mod data;
pub mod magic_finder;
pub mod metadata;
pub mod readers;
pub mod zip_archive;
pub mod zip_iterable;
pub mod zipfile;
pub use config::{ArchiveOffset, Config};
pub use data::ZipFileDataReadExt;
pub use metadata::{CentralDirectoryInfo, central_header_to_zip_file_inner};
pub use zip_archive::{ZipArchive, ZipArchiveMetadata};
pub use zip_iterable::{ZipIterable, ZipIterableFiles};
pub use zipfile::{ZipFile, ZipFileEntry, ZipFileEntryWithDataDescriptor, ZipFileSeek};
pub mod stream;
pub use stream::{
    ZipStreamReader, ZipStreamVisitor, read_zipfile_from_stream,
    read_zipfile_from_stream_with_compressed_size, read_zipfile_from_stream_with_options,
};
