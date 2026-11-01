#![doc = include_str!("../README.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(clippy::all, rust_2018_idioms)]
#![deny(
    clippy::all,
    clippy::missing_panics_doc,
    clippy::cargo,
    clippy::panic,
    clippy::cast_lossless,
    clippy::decimal_literal_representation,
    clippy::must_use_candidate
)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
#![allow(unexpected_cfgs)] // Needed for cfg(fuzzing)
#![allow(clippy::multiple_crate_versions)] // https://github.com/rust-lang/rust-clippy/issues/16440
pub use crate::compression::{CompressionMethod, SUPPORTED_COMPRESSION_METHODS};
pub use crate::datetime::DateTime;
pub use crate::format::aes::AesMode;
pub use crate::format::system::System;
pub use crate::format::{ZIP64_BYTES_THR, ZIP64_ENTRY_THR};
pub use crate::read::HasZipMetadata;
pub use crate::read::ZipReadOptions;

#[cfg(feature = "aes-crypto")]
#[allow(missing_docs)]
pub mod aes;
#[cfg(feature = "aes-crypto")]
pub use aes::AesSalt;
#[cfg(feature = "aes-crypto")]
mod aes_ctr;
pub mod compression;
#[doc(hidden)]
pub mod cp437;
#[doc(hidden)]
pub mod crc32;
pub mod datetime;
pub mod extra_fields;
pub mod format;
pub mod read;
pub mod result;
#[allow(missing_docs)]
pub mod types;
pub mod write;
#[allow(missing_docs)]
pub mod zipcrypto;
pub use extra_fields::ExtraField;

pub mod unstable;
