//! Shared ZIP writing option types.

pub mod options;
pub use options::sealed::FileOptionExtension;
pub use options::{ExtendedFileOptions, FileOptions, FullFileOptions, SimpleFileOptions};
