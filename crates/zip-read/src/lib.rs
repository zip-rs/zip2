//! ZIP archive reading APIs.

#![allow(unexpected_cfgs)]

pub use zip_core_lib::HasZipMetadata;
pub use zip_core_lib::crc32;
#[cfg(feature = "legacy-zip")]
pub mod legacy;
pub use zip_core_lib::unstable;

pub mod format {
    pub use zip_core_lib::format::*;
    pub mod find_central_directory;
    pub use find_central_directory::{MagicPositionCache, find_central_directory_end};
}
#[cfg(feature = "aes-crypto")]
pub use zip_core_lib::{AesSalt, aes};
pub use zip_core_lib::{
    CompressionMethod, DateTime, ExtraField, System, ZIP64_BYTES_THR, datetime, extra_fields,
    result, types, write, zipcrypto,
};

pub mod decompress;
pub use decompress::{Decompressor, InstantiationParameters, LazyDecompressor};
pub mod compression;
pub mod read;
pub use read::*;
