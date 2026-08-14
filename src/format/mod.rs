//! Zip format

pub(crate) mod aes;
pub(crate) mod blocks;
pub(crate) mod flags;
pub(crate) mod functions;
pub(crate) mod magic;

pub(crate) mod ffi {
    /// Regular
    pub const S_IFREG: u32 = 0b1000_0000_0000_0000; // 0o0_100_000
    /// Directory
    pub const S_IFDIR: u32 = 0b0100_0000_0000_0000; // 0o0_040_000
    /// Symbolic link
    pub const S_IFLNK: u32 = 0b1010_0000_0000_0000; // 0o0_120_000
}

/// The file size at which a ZIP64 record becomes necessary.
///
/// If a file larger than this threshold attempts to be written, compressed or uncompressed, and
/// [`FileOptions::large_file()`](crate::write::FileOptions::large_file) was not true, then [`crate::ZipWriter`] will
/// raise an [`io::Error`] with [`io::ErrorKind::Other`].
///
/// If the zip file itself is larger than this value, then a zip64 central directory record will be
/// written to the end of the file.
///
///```
/// # fn main() -> Result<(), zip::result::ZipError> {
/// # #[cfg(target_pointer_width = "64")]
/// # #[cfg(all(target_endian = "little", not(miri)))] // too long to run on miri
/// # {
/// use std::io::{self, Cursor, Write};
/// use std::error::Error;
/// use zip::{ZipWriter, write::SimpleFileOptions};
///
/// let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
/// // Writing an extremely large file for this test is faster without compression.
///
/// let big_len: usize = (zip::ZIP64_BYTES_THR as usize) + 1;
/// let big_buf = vec![0u8; big_len];
/// {
///     let options = SimpleFileOptions::default()
///         .compression_method(zip::CompressionMethod::Stored);
///     zip.start_file("zero.dat", options)?;
///     // This is too big!
///     let res = zip.write_all(&big_buf[..]).err().unwrap();
///     assert_eq!(res.kind(), io::ErrorKind::Other);
///     let description = format!("{}", &res);
///     assert_eq!(description, "Large file option has not been set");
///     // Attempting to write anything further to the same zip will still succeed, but the previous
///     // failing entry has been removed.
///     zip.start_file("one.dat", options)?;
///     let zip = zip.finish_into_readable()?;
///     let names: Vec<_> = zip.file_names().collect();
///     assert_eq!(names[0].as_ref().unwrap(), "one.dat");
/// }
///
/// // Create a new zip output.
/// let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
/// // This time, create a zip64 record for the file.
/// let options = SimpleFileOptions::default()
///      .compression_method(zip::CompressionMethod::Stored)
///      .large_file(true);
/// zip.start_file("zero.dat", options)?;
/// // This succeeds because we specified that it could be a large file.
/// assert!(zip.write_all(&big_buf[..]).is_ok());
/// # }
/// # Ok(())
/// # }
///```
pub const ZIP64_BYTES_THR: u64 = u32::MAX as u64;
pub const ZIP64_BYTES_THR_U32: u32 = u32::MAX;
/// The number of entries within a single zip necessary to allocate a zip64 central
/// directory record.
///
/// If more than this number of entries is written to a [`crate::ZipWriter`], then [`crate::ZipWriter::finish()`]
/// will write out extra zip64 data to the end of the zip file.
pub const ZIP64_ENTRY_THR: usize = u16::MAX as usize;
