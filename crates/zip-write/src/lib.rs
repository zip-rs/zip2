//! ZIP archive writing APIs.

mod write_support;
pub use write_support::ZipFileWriteExt;
#[path = "write.rs"]
pub mod write;
pub use write::*;
pub use zip_core_lib::{AesMode, CompressionMethod, DateTime, ExtraField, System};
