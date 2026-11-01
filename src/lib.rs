//! Backward-compatible facade for the ZIP workspace crates.
//!
//! The historical `zip` crate name and all of its public API paths remain available.
#![doc = include_str!("../README.md")]

pub use zip_add_file as add_file;
pub use zip_add_file::{PreparedZipFilePathExt, ZipAddFile, ZipFileBuilderPathExt};
pub use zip_core_lib::*;
pub use zip_extract as extract;
pub use zip_extract::path;
pub use zip_extract::{ZipExtract, ZipStreamExtract};
pub use zip_extract::{ZipFileDataPathExt, ZipFilePathExt};
pub use zip_read as reader;
#[cfg(feature = "legacy-zip")]
pub use zip_read::legacy;
pub use zip_read::read;
pub use zip_read::read::ZipFileDataReadExt;
pub use zip_read::{ZipArchive, ZipFile};
pub use zip_update as update;
pub use zip_update::ZipUpdate;
pub use zip_write::ZipFileWriteExt;
pub use zip_write::write;
pub use zip_write::{PreparedZipFile, ZipWriter};

/// ZIP format structures and read-side format scanning APIs.
pub mod format {
    pub use zip_core_lib::format::*;
    pub use zip_read::format::{MagicPositionCache, find_central_directory_end};
    pub mod find_central_directory {
        pub use zip_read::format::find_central_directory::*;
    }
}

/// Unstable APIs preserved from the original `zip` crate.
pub mod unstable {
    pub use zip_core_lib::unstable::*;

    /// Iterable ZIP reading that does not build an in-memory index of every entry.
    pub mod read {
        pub use zip_read::read::{ZipIterable, ZipIterableFiles};
    }

    /// Provides high level API for reading from a stream.
    pub mod stream {
        pub use zip_read::read::stream::{ZipStreamReader, ZipStreamVisitor};
    }
}
