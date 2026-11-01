//! Writing a ZIP archive

use crate::ZipFileWriteExt;
use zip_core_lib::ExtraField;
use zip_core_lib::compression::CompressionMethod;
use zip_core_lib::datetime::DateTime;
use zip_core_lib::extra_fields::ExtraFields;
use zip_core_lib::format::blocks::{
    Zip32CDEBlock, Zip32CentralDirectoryEnd, Zip64CentralDirectoryEnd,
    Zip64CentralDirectoryEndLocator, ZipLocalEntryBlock,
};
use zip_core_lib::format::ffi;
use zip_core_lib::format::flags::ZipFlags;
use zip_core_lib::format::magic::Magic;
use zip_core_lib::format::{ZIP64_BYTES_THR, ZIP64_BYTES_THR_U32, ZIP64_ENTRY_THR};
use zip_core_lib::result::{ZipError, ZipResult, invalid};
use zip_core_lib::types::{MIN_VERSION, ZipFileData, ZipRawValues};
use zip_core_lib::write::options::EncryptWith;

use core::default::Default;
use core::fmt::{Debug, Formatter};
use core::mem::{self, size_of};
use core::str::{Utf8Error, from_utf8};
use crc32fast::Hasher;
use indexmap::IndexMap;
use std::io::Cursor;
use std::io::ErrorKind;
use std::io::SeekFrom;
use std::io::{self, Read, Seek, Write};
#[cfg(feature = "rayon")]
use std::num::NonZeroUsize;

// re-export from types
pub use zip_core_lib::write::options::{
    ExtendedFileOptions, FileOptions, FullFileOptions, SimpleFileOptions,
};

#[allow(clippy::large_enum_variant)]
enum MaybeEncrypted<W> {
    Unencrypted(W),
    #[cfg(feature = "aes-crypto")]
    Aes(zip_core_lib::aes::AesWriter<W>),
    ZipCrypto(zip_core_lib::zipcrypto::ZipCryptoWriter<W>),
    /// Placeholder parked inside an idle, cached `flate2::write::DeflateEncoder` between DEFLATE
    /// entries (see [`DeflateCache`]). It discards anything written to it; the only bytes it can
    /// ever receive are the empty final block flate2's `Drop` emits when an idle encoder is
    /// dropped.
    #[cfg(feature = "deflate-flate2")]
    Closed {
        bytes_written: usize,
    },
}

impl<W: Write> MaybeEncrypted<W> {
    fn get_ref(&self) -> Option<&W> {
        match self {
            MaybeEncrypted::Unencrypted(w) => Some(w),
            #[cfg(feature = "aes-crypto")]
            MaybeEncrypted::Aes(w) => Some(w.get_ref()),
            MaybeEncrypted::ZipCrypto(w) => Some(w.get_ref()),
            #[cfg(feature = "deflate-flate2")]
            MaybeEncrypted::Closed { .. } => None,
        }
    }
    /// Returns a mutable reference to the underlying writer.
    ///
    /// # Safety
    /// The caller must preserve all invariants expected by the active wrapper
    /// (`Unencrypted`, `Aes`, or `ZipCrypto`):
    /// - Do not mutate the writer in a way that desynchronizes ZIP/encryption state.
    /// - Do not seek/write behind the wrapper's back such that subsequent writes
    ///   produce an invalid archive.
    /// - Ensure normal `&mut` exclusivity guarantees are upheld for the returned reference.
    unsafe fn get_mut(&mut self) -> Option<&mut W> {
        match self {
            MaybeEncrypted::Unencrypted(w) => Some(w),
            #[cfg(feature = "aes-crypto")]
            MaybeEncrypted::Aes(w) => Some(unsafe { w.get_mut() }),
            MaybeEncrypted::ZipCrypto(w) => Some(w.get_mut()),
            #[cfg(feature = "deflate-flate2")]
            MaybeEncrypted::Closed { .. } => None,
        }
    }
}

impl<W> Debug for MaybeEncrypted<W> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        // Don't print W, since it may be a huge Vec<u8>
        f.write_str(match self {
            MaybeEncrypted::Unencrypted(_) => "Unencrypted",
            #[cfg(feature = "aes-crypto")]
            MaybeEncrypted::Aes(_) => "AES",
            MaybeEncrypted::ZipCrypto(_) => "ZipCrypto",
            #[cfg(feature = "deflate-flate2")]
            MaybeEncrypted::Closed { .. } => "Closed",
        })
    }
}

impl<W: Write> Write for MaybeEncrypted<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            MaybeEncrypted::Unencrypted(w) => w.write(buf),
            #[cfg(feature = "aes-crypto")]
            MaybeEncrypted::Aes(w) => w.write(buf),
            MaybeEncrypted::ZipCrypto(w) => w.write(buf),
            #[cfg(feature = "deflate-flate2")]
            MaybeEncrypted::Closed { bytes_written } => {
                *bytes_written = (*bytes_written).saturating_add(buf.len());
                if *bytes_written > 2 {
                    Err(io::Error::new(
                        ErrorKind::FileTooLarge,
                        "More than 2 bytes unexpectedly written to Deflate encoder's closed internal writer",
                    ))
                } else {
                    Ok(buf.len())
                }
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            MaybeEncrypted::Unencrypted(w) => w.flush(),
            #[cfg(feature = "aes-crypto")]
            MaybeEncrypted::Aes(w) => w.flush(),
            MaybeEncrypted::ZipCrypto(w) => w.flush(),
            #[cfg(feature = "deflate-flate2")]
            MaybeEncrypted::Closed { .. } => Ok(()),
        }
    }
}

#[allow(clippy::large_enum_variant)]
enum GenericZipWriter<W: Write + Seek> {
    Closed,
    Storer(MaybeEncrypted<W>),
    #[cfg(feature = "deflate-flate2")]
    Deflater(flate2::write::DeflateEncoder<MaybeEncrypted<W>>),
    #[cfg(feature = "deflate-zopfli")]
    ZopfliDeflater(zopfli::DeflateEncoder<MaybeEncrypted<W>>),
    #[cfg(feature = "deflate-zopfli")]
    BufferedZopfliDeflater(std::io::BufWriter<zopfli::DeflateEncoder<MaybeEncrypted<W>>>),
    #[cfg(feature = "_bzip2_any")]
    Bzip2(bzip2::write::BzEncoder<MaybeEncrypted<W>>),
    #[cfg(feature = "zstd")]
    Zstd(zstd::stream::write::Encoder<'static, MaybeEncrypted<W>>),
    #[cfg(feature = "xz")]
    Xz(Box<lzma_rust2::XzWriter<MaybeEncrypted<W>>>),
    #[cfg(feature = "ppmd")]
    Ppmd(Box<ppmd_rust::Ppmd8Encoder<MaybeEncrypted<W>>>),
}

impl<W: Write + Seek> Debug for GenericZipWriter<W> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Closed => f.write_str("Closed"),
            Self::Storer(w) => f.write_fmt(format_args!("Storer({w:?})")),
            #[cfg(feature = "deflate-flate2")]
            Self::Deflater(w) => f.write_fmt(format_args!("Deflater({:?})", w.get_ref())),
            #[cfg(feature = "deflate-zopfli")]
            Self::ZopfliDeflater(_) => f.write_str("ZopfliDeflater"),
            #[cfg(feature = "deflate-zopfli")]
            Self::BufferedZopfliDeflater(_) => f.write_str("BufferedZopfliDeflater"),
            #[cfg(feature = "_bzip2_any")]
            Self::Bzip2(w) => f.write_fmt(format_args!("Bzip2({:?})", w.get_ref())),
            #[cfg(feature = "zstd")]
            Self::Zstd(w) => f.write_fmt(format_args!("Zstd({:?})", w.get_ref())),
            #[cfg(feature = "xz")]
            Self::Xz(w) => f.write_fmt(format_args!("Xz({:?})", w.inner())),
            #[cfg(feature = "ppmd")]
            Self::Ppmd(_) => f.write_fmt(format_args!("Ppmd8Encoder")),
        }
    }
}

// Put the struct declaration in a private module to convince rustdoc to display ZipWriter nicely
pub(crate) mod zip_writer {
    #[cfg(feature = "deflate-flate2")]
    use super::DeflateCache;
    use super::{GenericZipWriter, ZipWriterStats};
    use core::fmt::{Debug, Formatter};
    use indexmap::IndexMap;
    use std::io::{Seek, Write};
    use zip_core_lib::types::ZipFileData;

    /// ZIP archive generator
    ///
    /// Handles the bookkeeping involved in building an archive, and provides an
    /// API to edit its contents.
    ///
    /// ```
    /// # fn doit() -> zip::result::ZipResult<()>
    /// # {
    /// use zip::ZipWriter;
    /// use std::io::Write;
    /// use zip::write::SimpleFileOptions;
    ///
    /// // We use a cursor + vec here, though you'd normally use a `File`
    /// let mut cursor = std::io::Cursor::new(Vec::new());
    /// let mut archive = ZipWriter::new(&mut cursor);
    ///
    /// let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    /// archive.start_file("hello_world.txt", options)?;
    /// archive.write(b"Hello, World!")?;
    /// // also direct write with
    /// // std::io::copy(&mut file, &mut zip)?;
    ///
    /// // Apply the changes you've made.
    /// // Dropping the `ZipWriter` will have the same effect, but may silently fail
    /// archive.finish()?;
    ///
    /// // raw zip data is available as a Vec<u8>
    /// let zip_bytes = cursor.into_inner();
    ///
    /// # Ok(())
    /// # }
    /// # doit().unwrap();
    /// ```
    pub struct ZipWriter<W: Write + Seek> {
        pub(super) inner: GenericZipWriter<W>,
        pub(super) files: IndexMap<Box<[u8]>, ZipFileData>,
        pub(super) stats: ZipWriterStats,
        pub(super) writing_to_file: bool,
        pub(super) writing_raw: bool,
        pub(super) comment: Box<[u8]>,
        pub(super) zip64_extensible_data_sector: Option<Box<[u8]>>,
        pub(super) flush_on_finish_file: bool,
        pub(super) seek_possible: bool,
        pub(crate) auto_large_file: bool,
        /// Idle DEFLATE compressor kept between entries to avoid reallocating it (#633).
        #[cfg(feature = "deflate-flate2")]
        pub(super) deflate_cache: DeflateCache<W>,
    }

    impl<W: Write + Seek> Debug for ZipWriter<W> {
        fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
            f.write_fmt(format_args!(
                "ZipWriter {{files: {:?}, stats: {:?}, writing_to_file: {}, writing_raw: {}, comment: {:?}, flush_on_finish_file: {}}}",
                self.files, self.stats, self.writing_to_file, self.writing_raw,
                self.comment, self.flush_on_finish_file))
        }
    }

    impl<W: Write + Seek> ZipWriter<W> {
        /// Gets a reference to the underlying writer in this `ZipWriter`.
        pub fn get_ref(&self) -> Option<&W> {
            match &self.inner {
                GenericZipWriter::Closed => None,
                GenericZipWriter::Storer(w) => w.get_ref(),
                #[cfg(feature = "deflate-flate2")]
                GenericZipWriter::Deflater(w) => w.get_ref().get_ref(),
                #[cfg(feature = "deflate-zopfli")]
                GenericZipWriter::ZopfliDeflater(w) => w.get_ref().get_ref(),
                #[cfg(feature = "deflate-zopfli")]
                GenericZipWriter::BufferedZopfliDeflater(w) => w.get_ref().get_ref().get_ref(),
                #[cfg(feature = "_bzip2_any")]
                GenericZipWriter::Bzip2(w) => w.get_ref().get_ref(),
                #[cfg(feature = "zstd")]
                GenericZipWriter::Zstd(w) => w.get_ref().get_ref(),
                #[cfg(feature = "xz")]
                GenericZipWriter::Xz(w) => w.inner().get_ref(),
                #[cfg(feature = "ppmd")]
                GenericZipWriter::Ppmd(w) => w.get_ref().get_ref(),
            }
        }

        /// Gets a reference to the underlying writer in this `ZipWriter`.
        ///
        /// # Safety
        ///
        /// Caller must not corrupt the archive, and must seek back to the current position
        /// before continuing to write to the `ZipWriter`.
        pub unsafe fn get_mut(&mut self) -> Option<&mut W> {
            unsafe {
                match &mut self.inner {
                    GenericZipWriter::Closed => None,
                    GenericZipWriter::Storer(w) => w.get_mut(),
                    #[cfg(feature = "deflate-flate2")]
                    GenericZipWriter::Deflater(w) => w.get_mut().get_mut(),
                    #[cfg(feature = "deflate-zopfli")]
                    GenericZipWriter::ZopfliDeflater(w) => w.get_mut().get_mut(),
                    #[cfg(feature = "deflate-zopfli")]
                    GenericZipWriter::BufferedZopfliDeflater(w) => w.get_mut().get_mut().get_mut(),
                    #[cfg(feature = "_bzip2_any")]
                    GenericZipWriter::Bzip2(w) => w.get_mut().get_mut(),
                    #[cfg(feature = "zstd")]
                    GenericZipWriter::Zstd(w) => w.get_mut().get_mut(),
                    #[cfg(feature = "xz")]
                    GenericZipWriter::Xz(w) => w.inner_mut().get_mut(),
                    #[cfg(feature = "ppmd")]
                    GenericZipWriter::Ppmd(w) => w.get_mut().get_mut(),
                }
            }
        }
    }
}
use zip_core_lib::CompressionMethod::Stored;
use zip_core_lib::result::ZipError::UnsupportedArchive;
#[doc(inline)]
pub use zip_core_lib::write::FileOptionExtension;
use zip_core_lib::zipcrypto::CHUNK_SIZE;
pub use zip_writer::ZipWriter;

#[derive(Default, Debug)]
struct ZipWriterStats {
    hasher: Hasher,
    start: u64,
    bytes_written: u64,
}

impl<W: Write + Seek> Write for ZipWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.writing_to_file {
            return Err(io::Error::other("No file has been started"));
        }
        if buf.is_empty() {
            return Ok(0);
        }
        match self.inner.ref_mut() {
            Some(ref mut w) => {
                let write_result = w.write(buf);
                if let Ok(count) = write_result {
                    self.stats.update(&buf[..count]);
                    // Only perform the expensive large-file check when we first cross the threshold.
                    if self.stats.bytes_written >= ZIP64_BYTES_THR {
                        let is_large_file = self
                            .files
                            .last()
                            .ok_or_else(|| std::io::Error::other("Cannot get last file"))?
                            .1
                            .large_file;
                        if !is_large_file {
                            return Err(if let Err(e) = self.abort_file() {
                                let abort_io_err: io::Error = e.into();
                                io::Error::new(
                                    abort_io_err.kind(),
                                    format!(
                                        "Large file option has not been set and abort_file() failed: {abort_io_err}"
                                    ),
                                )
                            } else {
                                io::Error::other("Large file option has not been set")
                            });
                        }
                    }
                }
                write_result
            }
            None => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "write(): ZipWriter was already closed",
            )),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.inner.ref_mut() {
            Some(ref mut w) => w.flush(),
            None => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "flush(): ZipWriter was already closed",
            )),
        }
    }
}

impl ZipWriterStats {
    fn update(&mut self, buf: &[u8]) {
        self.hasher.update(buf);
        self.bytes_written += buf.len() as u64;
    }
}

impl<A: Read + Write + Seek> ZipWriter<A> {
    /// Constructs a writer from parsed archive metadata for use by update extensions.
    #[doc(hidden)]
    pub fn from_append_metadata(
        readwriter: A,
        files: IndexMap<Box<[u8]>, ZipFileData>,
        comment: Box<[u8]>,
        zip64_extensible_data_sector: Option<Box<[u8]>>,
    ) -> ZipWriter<A> {
        ZipWriter {
            inner: GenericZipWriter::Storer(MaybeEncrypted::Unencrypted(readwriter)),
            files,
            stats: ZipWriterStats::default(),
            writing_to_file: false,
            comment,
            zip64_extensible_data_sector,
            writing_raw: true, // avoid recomputing the last file's header
            flush_on_finish_file: false,
            seek_possible: true,
            auto_large_file: false,
            #[cfg(feature = "deflate-flate2")]
            deflate_cache: DeflateCache::default(),
        }
    }

    #[doc(hidden)]
    pub fn with_file_data_and_inner<T>(
        &mut self,
        name: &[u8],
        f: impl FnOnce(&mut A, &mut ZipFileData) -> ZipResult<T>,
    ) -> ZipResult<T> {
        self.finish_file()?;
        let index = self.index_by_name(name)?;
        let inner = self.inner.try_inner_mut()?;
        f(inner, &mut self.files[index])
    }

    #[doc(hidden)]
    pub fn deep_copy_file_from_data_start(
        &mut self,
        src_name: &str,
        dest_name: &str,
        src_data_start: u64,
    ) -> ZipResult<()> {
        self.finish_file()?;
        if src_name == dest_name || self.files.contains_key(dest_name.as_bytes()) {
            return Err(invalid!("That file already exists"));
        }
        let write_position = self.inner.try_inner_mut()?.stream_position()?;
        let src_index = self.index_by_name(src_name.as_bytes())?;
        let src_data = &mut self.files[src_index];
        let available = write_position
            .checked_sub(src_data_start)
            .ok_or(invalid!("File data starts past the end of the archive"))?;
        let mut compressed_size = src_data.compressed_size;
        if compressed_size > available {
            compressed_size = available;
            src_data.compressed_size = compressed_size;
        }
        self.inner
            .try_inner_mut()?
            .seek(SeekFrom::Start(write_position))?;
        let mut new_data = src_data.clone();
        let dest_name_raw = dest_name.as_bytes();
        new_data.header_start = write_position;
        new_data.central_header_start = 0;
        new_data.set_language_encoding_for(dest_name_raw);
        new_data.drop_unicode_path_renaming(dest_name_raw);
        let index = self.insert_file_data(dest_name_raw, new_data)?;
        let new_data = &mut self.files[index];
        let result: io::Result<()> = {
            let plain_writer = self.inner.try_inner_mut()?;
            new_data.write_local_header(write_position, plain_writer, dest_name_raw, None)?;
            debug_assert_eq!(
                new_data.data_start.get(),
                Some(&plain_writer.stream_position()?)
            );
            self.writing_to_file = true;
            let mut buffer = [0u8; 64 * 1024];
            let mut src_pos = src_data_start;
            let mut dst_pos = plain_writer.stream_position()?;
            let mut remaining = compressed_size;
            while remaining != 0 {
                let n = remaining.min(buffer.len() as u64) as usize;
                plain_writer.seek(SeekFrom::Start(src_pos))?;
                plain_writer.read_exact(&mut buffer[..n])?;
                plain_writer.seek(SeekFrom::Start(dst_pos))?;
                plain_writer.write_all(&buffer[..n])?;
                src_pos = src_pos
                    .checked_add(n as u64)
                    .ok_or_else(|| io::Error::other("source offset overflows u64"))?;
                dst_pos = dst_pos
                    .checked_add(n as u64)
                    .ok_or_else(|| io::Error::other("destination offset overflows u64"))?;
                remaining -= n as u64;
            }
            if self.flush_on_finish_file {
                plain_writer.flush()?;
            }
            Ok(())
        };
        self.ok_or_abort_file(result)?;
        self.writing_to_file = false;
        Ok(())
    }

    #[doc(hidden)]
    pub fn archive_append_position(&mut self) -> ZipResult<u64> {
        self.finish_file()?;
        Ok(self.inner.try_inner_mut()?.stream_position()?)
    }

    #[doc(hidden)]
    pub fn append_archive_contents<R: Read>(
        &mut self,
        reader: &mut R,
        length: u64,
    ) -> ZipResult<()> {
        self.writing_to_file = true;
        self.writing_raw = true;
        let mut limited = reader.take(length);
        io::copy(&mut limited, self.inner.try_inner_mut()?)?;
        Ok(())
    }

    #[doc(hidden)]
    pub fn append_archive_files(&mut self, files: IndexMap<Box<[u8]>, ZipFileData>) {
        self.files.extend(files);
    }

    /// `flush_on_finish_file` is designed to support a streaming `inner` that may unload flushed
    /// bytes. It flushes a file's header and body once it starts writing another file. A `ZipWriter`
    /// will not try to seek back into where a previous file was written unless
    /// either [`ZipWriter::abort_file`] is called while [`ZipWriter::is_writing_file`] returns
    /// false, or `ZipUpdate::deep_copy_file` is called. In the latter case, it will only need to
    /// read previously-written files and not overwrite them.
    ///
    /// Note: when using an `inner` that cannot overwrite flushed bytes, do not wrap it in a
    /// [`std::io::BufWriter`], because that has a [`Seek::seek`] method that implicitly calls
    /// [`std::io::BufWriter::flush`], and `ZipWriter` needs to seek backward to update each file's header with
    /// the size and checksum after writing the body.
    ///
    /// This setting is false by default.
    pub fn set_flush_on_finish_file(&mut self, flush_on_finish_file: bool) {
        self.flush_on_finish_file = flush_on_finish_file;
    }
}

#[cfg(not(feature = "unreserved"))]
fn validate_extra_fields(
    extra_fields: &[zip_core_lib::extra_fields::CustomExtraField],
) -> ZipResult<()> {
    use zip_core_lib::extra_fields::EXTRA_FIELD_MAPPING;
    use zip_core_lib::extra_fields::UsedExtraField;
    for x in extra_fields.iter() {
        let header_id = x.header_id;
        if UsedExtraField::try_from(header_id).is_err() && EXTRA_FIELD_MAPPING.contains(&header_id)
        {
            return Err(ZipError::Io(io::Error::other(format!(
                "Extra data header ID {header_id:#06} (0x{header_id:x}) \
                        requires crate feature \"unreserved\"",
            ))));
        }
    }
    Ok(())
}

impl<W: Write + Seek> ZipWriter<W> {
    /// Initializes the archive.
    ///
    /// Before writing to this object, the [`ZipWriter::start_file`] function should be called.
    /// After a successful write, the file remains open for writing. After a failed write, call
    /// [`ZipWriter::is_writing_file`] to determine if the file remains open.
    pub fn new(inner: W) -> ZipWriter<W> {
        ZipWriter {
            inner: GenericZipWriter::Storer(MaybeEncrypted::Unencrypted(inner)),
            files: IndexMap::new(),
            stats: ZipWriterStats::default(),
            writing_to_file: false,
            writing_raw: false,
            comment: Box::new([]),
            zip64_extensible_data_sector: None,
            flush_on_finish_file: false,
            seek_possible: true,
            auto_large_file: false,
            #[cfg(feature = "deflate-flate2")]
            deflate_cache: DeflateCache::default(),
        }
    }

    /// Set automatically large file to true if needed
    #[must_use]
    pub fn set_auto_large_file(mut self) -> Self {
        self.auto_large_file = true;
        self
    }

    /// Returns true if a file is currently open for writing.
    pub const fn is_writing_file(&self) -> bool {
        self.writing_to_file && !self.inner.is_closed()
    }

    /// Set ZIP archive comment.
    pub fn set_comment<S>(&mut self, comment: S) -> ZipResult<()>
    where
        S: Into<Box<str>>,
    {
        self.set_raw_comment(comment.into().into_boxed_bytes())
    }

    /// Set raw ZIP archive comment.
    ///
    /// This sets the raw bytes of the comment.
    /// The comment is typically expected to be encoded in UTF-8.
    /// If the comment is more than `u16::MAX` it will be truncated
    pub fn set_raw_comment(&mut self, comment: Box<[u8]>) -> ZipResult<()> {
        let max_comment_len = u16::MAX as usize; // 65,535
        if comment.len() <= max_comment_len {
            self.comment = comment;
            return Ok(());
        }
        let (allowed, rest) = comment.split_at(max_comment_len);
        self.comment = allowed.into();
        Err(ZipError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Unexpected data comment - {} bytes too long", rest.len()),
        )))
    }

    /// Get ZIP archive comment.
    pub fn get_comment(&self) -> Result<&str, Utf8Error> {
        from_utf8(self.get_raw_comment())
    }

    /// Get raw ZIP archive comment.
    ///
    /// This returns the raw bytes of the comment. The comment
    /// is typically expected to be encoded in UTF-8.
    pub const fn get_raw_comment(&self) -> &[u8] {
        &self.comment
    }

    /// Get the zip64 extensible data. Use at your own risk
    /// See 4.3.14.3, 4.3.14.4, 4.4.27 and APPENDIX C of the specification
    pub fn set_raw_zip64_extensible_data_sector(&mut self, extensible_data: Box<[u8]>) {
        self.zip64_extensible_data_sector = Some(extensible_data);
    }

    /// Get raw ZIP64 archive comment.
    ///
    /// This returns the raw bytes of the comment. The comment
    /// is typically expected to be encoded in UTF-8.
    #[deprecated(
        note = "Zip64 comment is not part of the zip specification - see https://github.com/zip-rs/zip2/pull/747"
    )]
    pub fn get_raw_zip64_comment(&self) -> Option<&[u8]> {
        // no-op since deprecated
        None
    }

    /// Set the file length and crc32 manually.
    ///
    /// # Safety
    ///
    /// This function assumes that `length` and `crc32` accurately describe the data that has
    /// been (or will be) written for the currently open file entry.
    ///
    /// The caller must ensure that:
    /// - A file entry is currently being written (that is, [`Self::start_file`] or equivalent has
    ///   been called successfully and `abort_file` has not been called since).
    /// - `length` is the exact uncompressed size, in bytes, of the file data written to the
    ///   underlying [`Write`] implementation for this entry.
    /// - `crc32` is the correct CRC-32 checksum for that same data, in the format expected by
    ///   the ZIP specification.
    ///
    /// If these requirements are not met, the generated ZIP archive may contain inconsistent
    /// or corrupt metadata, which can cause readers to report errors, skip data, or accept
    /// data whose integrity cannot be verified.
    ///
    /// This overwrites the internal crc32 calculation. It should only be used in case
    /// the underlying [Write] is written independently and you need to adjust the zip metadata.
    pub unsafe fn set_file_metadata(&mut self, length: u64, crc32: u32) -> ZipResult<()> {
        if !self.writing_to_file {
            return Err(ZipError::Io(io::Error::other("No file has been started")));
        }
        self.stats.hasher = Hasher::new_with_initial_len(crc32, length);
        self.stats.bytes_written = length;
        Ok(())
    }

    fn ok_or_abort_file<T, E: Into<ZipError>>(&mut self, result: Result<T, E>) -> ZipResult<T> {
        match result {
            Err(e) => {
                let _ = self.abort_file();
                Err(e.into())
            }
            Ok(t) => Ok(t),
        }
    }

    /// Start a new file for with the requested options.
    fn start_entry<T: FileOptionExtension>(
        &mut self,
        file_name_raw: &[u8],
        mut options: FileOptions<'_, '_, T>,
        raw_values: Option<ZipRawValues>,
    ) -> ZipResult<()> {
        self.finish_file()?;

        let header_start = self.inner.try_inner_mut()?.stream_position()?;
        #[cfg(not(feature = "unreserved"))]
        {
            if raw_values.is_none()
                && let Some(extra_fields) = options.extended_options.extra_fields()
            {
                validate_extra_fields(extra_fields)?;
            }
        }
        let raw_values = raw_values.unwrap_or(ZipRawValues {
            crc32: 0,
            compressed_size: 0,
            uncompressed_size: 0,
            language_encoding: None,
        });
        if options.extended_options.validate_extra_fields()
            && let Some(extra_fields) = options.extended_options.extra_fields()
        {
            for x in extra_fields.iter() {
                if !ExtendedFileOptions::extra_field_parses(x.header_id, &x.data, x.central_only) {
                    return Err(invalid!(
                        "Extra field {:#06x} would not be readable: malformed data",
                        x.header_id
                    ));
                }
                // A reader replaces the entry name with a valid Info-ZIP Unicode Path field's
                // content (APPNOTE 4.6.9), so it must not name a different file than this one.
                if x.header_id
                    == zip_core_lib::format::extra_fields::ExtraFieldId::UnicodePath.as_u16()
                    && let Some(content) = x.data.get(5..)
                    && x.data[1..5] == crc32fast::hash(file_name_raw).to_le_bytes()
                    && core::str::from_utf8(content).is_ok()
                    && content != file_name_raw
                {
                    return Err(invalid!(
                        "Extra field {:#06x} would not be readable: renames the entry",
                        x.header_id
                    ));
                }
            }
        }
        let extra_fields = match options.extended_options.extra_fields() {
            Some(data) => data.iter().map(|x| ExtraField::Custom(x.clone())).collect(),
            None => vec![],
        };
        let entry_len: usize = mem::size_of::<Magic>()
            + mem::size_of::<ZipLocalEntryBlock>()
            + file_name_raw.len()
            + extra_fields.iter().map(|x| x.size(false)).sum::<usize>();
        if entry_len >= (u16::MAX as usize) {
            return Err(invalid!(
                "ZipLocalBlock + filename + extra fields must be less than 64KiB when combined"
            ));
        }

        let mut file = ZipFileData::initialize_local_block(
            file_name_raw,
            &options,
            &raw_values,
            header_start,
            ExtraFields {
                inner: extra_fields,
            },
        );
        if let Some(comment) = options.extended_options.take_file_comment() {
            if comment.len() > u16::MAX as usize {
                return Err(invalid!("File comment must be less than 64KiB"));
            }
            if !comment.is_ascii() {
                file.flags |= ZipFlags::LanguageEncoding.as_u16();
            }
            file.file_comment = comment;
        }
        if let Some(language_encoding) = raw_values.language_encoding {
            file.flags
                .set(ZipFlags::LanguageEncoding, language_encoding);
        }
        if (!self.seek_possible || matches!(options.encrypt_with, Some(EncryptWith::ZipCrypto(..))))
            && file
                .unix_mode()
                .is_some_and(|p| p & ffi::S_IFDIR != ffi::S_IFDIR)
        {
            file.flags |= ZipFlags::UsingDataDescriptor.as_u16();
        }
        file.version_made_by = file.version_made_by.max(file.version_needed() as u8);
        let index = self.insert_file_data(file_name_raw, file)?;
        self.writing_to_file = true;
        let result: ZipResult<()> = {
            let file = &mut self.files[index];
            let writer = self.inner.try_inner_mut()?;
            file.write_local_header(
                header_start,
                writer,
                file_name_raw,
                Some((options.alignment, options.alignment_header_id)),
            )?;
            Ok(())
        };
        self.ok_or_abort_file(result)?;
        self.stats.start = self.inner.try_inner_mut()?.stream_position()?;
        match options.encrypt_with {
            #[cfg(feature = "aes-crypto")]
            Some(EncryptWith::Aes {
                mode,
                // If the password is None (when we use the `options()` method)
                // we re-use the same encryption
                password: Some(password),
                salt,
                ..
            }) => {
                let writer = self.close_writer()?;
                let aeswriter =
                    zip_core_lib::aes::AesWriter::new_with_options(writer, mode, password, salt)?;
                self.inner = GenericZipWriter::Storer(MaybeEncrypted::Aes(aeswriter));
            }
            Some(EncryptWith::ZipCrypto(keys, ..)) => {
                let writer = self.close_writer()?;
                let file = &mut self.files[index];
                // With ZipCrypto, we _need_ to use a data descriptor so that
                // we can initialize the stream properly.
                let mut zipwriter = zip_core_lib::zipcrypto::ZipCryptoWriter {
                    writer,
                    keys,
                    buffer: [0u8; CHUNK_SIZE],
                };
                self.stats.start = zipwriter.writer.stream_position()?;
                // crypto_header is counted as part of the data
                let mut crypto_header = [0u8; 12];
                // The last two bytes of the header should either be the CRC of
                // the file _OR_ the last timepart of the last modified time if
                // a data descriptor is used.
                //
                // However, this header is encrypted, and ZipCrypto is not a
                // seekable encryption algorithm - an earlier plaintext byte
                // will have an impact on the later values.
                //
                // This makes it impossible to write the file without first
                // calculating its CRC. To avoid having to read the file twice
                // or keeping the entire file in memory, we force all ZipCrypto
                // files to use a data descriptor. This way, we can use the
                // local_modified_time as a password check byte instead of the
                // CRC.
                crypto_header[10..=11].copy_from_slice(
                    &file
                        .last_modified_time
                        .unwrap_or_else(DateTime::default_for_write)
                        .timepart()
                        .to_le_bytes(),
                );
                let result = zipwriter.write_all(&crypto_header);
                self.ok_or_abort_file(result)?;
                self.inner = GenericZipWriter::Storer(MaybeEncrypted::ZipCrypto(zipwriter));
            }
            _ => {}
        }
        let file = &mut self.files[index];
        //debug_assert!(file.data_start.get().is_none());
        file.data_start.get_or_init(|| self.stats.start);
        self.stats.bytes_written = 0;
        self.stats.hasher = Hasher::new();
        Ok(())
    }

    fn insert_file_data(&mut self, file_name_raw: &[u8], file: ZipFileData) -> ZipResult<usize> {
        if self.files.contains_key(file_name_raw) {
            return Err(invalid!(
                "Duplicate filename: {:?}",
                file_name_raw.escape_ascii()
            ));
        }
        let (index, _) = self.files.insert_full(file_name_raw.into(), file);
        Ok(index)
    }

    /// [`GenericZipWriter::switch_to`] with this writer's DEFLATE encoder cache.
    fn switch_inner(&mut self, make_new_self: SwitchWriterFunction<W>) -> ZipResult<()> {
        #[cfg(feature = "deflate-flate2")]
        let cache = &mut self.deflate_cache;
        #[cfg(not(feature = "deflate-flate2"))]
        let cache = &mut DeflateCache::default();
        self.inner.switch_to(cache, make_new_self)
    }

    fn finish_file(&mut self) -> ZipResult<()> {
        if !self.writing_to_file {
            return Ok(());
        }

        let make_plain_writer = self.inner.prepare_next_writer(
            Stored,
            None,
            #[cfg(feature = "deflate-zopfli")]
            None,
        )?;
        self.switch_inner(make_plain_writer)?;
        self.switch_to_non_encrypting_writer()?;
        let writer = self.inner.try_inner_mut()?;

        if !self.writing_raw {
            let Some((file_name_raw, file)) = self.files.last_mut() else {
                return Ok(());
            };
            file.uncompressed_size = self.stats.bytes_written;

            let file_end = writer.stream_position()?;
            debug_assert!(file_end >= self.stats.start);
            file.compressed_size = file_end - self.stats.start;

            if !file.is_using_data_descriptor() {
                // Not using a data descriptor means the underlying writer
                // supports seeking, so we can go back and update the AES Extra
                // Data header to use AE1 for large files.
                #[cfg(feature = "aes-crypto")]
                update_aes_extra_field(writer, file, self.stats.bytes_written)?;
            }

            file.crc32 = if file
                .aes_settings()
                .is_some_and(|(_, vendor_version)| vendor_version.is_ae2_encrypted())
            {
                // AE2 disables CRC32 in the local file header
                0
            } else {
                self.stats.hasher.clone().finalize()
            };

            if file.is_using_data_descriptor() {
                file.write_data_descriptor(writer, self.auto_large_file)?;
            } else {
                file.update_local_file_header(writer, file_name_raw)?;
                writer.seek(SeekFrom::Start(file_end))?;
            }
        }
        if self.flush_on_finish_file {
            let result = writer.flush();
            self.ok_or_abort_file(result)?;
        }

        self.writing_to_file = false;
        Ok(())
    }

    fn switch_to_non_encrypting_writer(&mut self) -> Result<(), ZipError> {
        self.inner = GenericZipWriter::Storer(
            match mem::replace(&mut self.inner, GenericZipWriter::Closed) {
                #[cfg(feature = "aes-crypto")]
                GenericZipWriter::Storer(MaybeEncrypted::Aes(writer)) => {
                    MaybeEncrypted::Unencrypted(writer.finish()?)
                }
                GenericZipWriter::Storer(MaybeEncrypted::ZipCrypto(writer)) => {
                    MaybeEncrypted::Unencrypted(writer.finish()?)
                }
                GenericZipWriter::Storer(MaybeEncrypted::Unencrypted(w)) => {
                    MaybeEncrypted::Unencrypted(w)
                }
                #[cfg(feature = "deflate-flate2")]
                GenericZipWriter::Storer(MaybeEncrypted::Closed { .. }) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "ZipWriter's inner writer is not available",
                    )
                    .into());
                }
                _ => unreachable!(
                    "switch_to_non_encrypting_writer called with incompatible GenericZipWriter variant"
                ),
            },
        );
        Ok(())
    }

    /// Removes an entry from the archive's directory without rewriting the
    /// archive.
    ///
    /// The entry stops appearing in the archive: it is dropped from the central
    /// directory written by [`ZipWriter::finish`], so readers no longer see it.
    /// Its local header and file data are **left in place** as unreferenced
    /// bytes, which is what makes this cheap — nothing after the removed entry
    /// is moved or rewritten, so the cost does not scale with the size of the
    /// archive.
    ///
    /// The file therefore does not shrink. Reclaiming the space needs a
    /// rewrite, which a caller can do when it is worth doing (for example by
    /// copying the surviving entries into a fresh archive with
    /// `ZipUpdate::merge_archive`) rather than on every removal.
    ///
    /// Any entry that shares data with the removed one — see
    /// [`ZipWriter::shallow_copy_file`] — keeps working, because the bytes it
    /// points at are untouched.
    ///
    /// If a file is currently being written, it is finished first, exactly as
    /// [`ZipWriter::start_file`] does.
    ///
    /// # Safety
    ///
    ///.This function only deletes the file entry from the archive's central
    /// directory. The file's content remains untouched.
    ///
    /// # Errors
    ///
    /// Returns [`ZipError::FileNotFound`] if `name` does not match an entry.
    ///
    /// ```
    /// # fn main() -> Result<(), zip::result::ZipError> {
    /// # #[cfg(any(feature = "deflate-flate2", not(feature = "_deflate-any")))]
    /// # {
    /// use std::io::{Cursor, Write};
    /// use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};
    ///
    /// let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    /// zip.start_file("keep.txt", SimpleFileOptions::default())?;
    /// zip.write_all(b"kept")?;
    /// zip.start_file("drop.txt", SimpleFileOptions::default())?;
    /// zip.write_all(b"dropped")?;
    /// zip.hide_file("drop.txt")?;
    ///
    /// let mut archive = ZipArchive::new(zip.finish()?)?;
    /// assert_eq!(archive.len(), 1);
    /// assert!(archive.by_name("keep.txt").is_ok());
    /// assert!(archive.by_name("drop.txt").is_err());
    /// # }
    /// # Ok(())
    /// # }
    /// ```
    pub fn hide_file(&mut self, name: &str) -> ZipResult<()> {
        self.finish_file()?;
        // `shift_remove` rather than `swap_remove`: the central directory is
        // written in `files` order, and reordering the surviving entries on
        // every removal would be a surprising side effect.
        self.files
            .shift_remove(name.as_bytes())
            .map(|_| ())
            .ok_or(ZipError::FileNotFound)
    }

    /// Removes the file currently being written from the archive if there is one, or else removes
    /// the file most recently written.
    pub fn abort_file(&mut self) -> ZipResult<()> {
        let (_, last_file) = self.files.pop().ok_or(ZipError::FileNotFound)?;
        let make_plain_writer = self.inner.prepare_next_writer(
            Stored,
            None,
            #[cfg(feature = "deflate-zopfli")]
            None,
        )?;
        self.switch_inner(make_plain_writer)?;
        self.switch_to_non_encrypting_writer()?;
        // Make sure this is the last file, and that no shallow copies of it remain; otherwise we'd
        // overwrite a valid file and corrupt the archive
        let last_file_start = last_file
            .data_start
            .get()
            .copied()
            .unwrap_or(last_file.header_start);
        let writer = self.inner.try_inner_mut()?;
        let current_position = writer.stream_position()?;
        let rewind_safe = last_file.header_start <= current_position
            && last_file_start <= current_position
            && self.files.values().all(|file| {
                file.data_start.get().copied().unwrap_or(file.header_start) < last_file_start
            });
        if rewind_safe {
            writer.seek(SeekFrom::Start(last_file.header_start))?;
        }
        self.writing_to_file = false;
        Ok(())
    }

    /// Create a file in the archive and start writing its contents. The file must not have the
    /// same name as a file already in the archive.
    ///
    /// The data should be written using the [`Write`] implementation on this [`ZipWriter`]
    pub fn start_file<S: ToString, T: FileOptionExtension>(
        &mut self,
        name: S,
        options: FileOptions<'_, '_, T>,
    ) -> ZipResult<()> {
        let name = name.to_string();
        let options = options.with_name(name.as_bytes());
        self.start_file_with_options(options)
    }

    /// Create a file in the archive and start writing its contents. The file must not have the
    /// same name as a file already in the archive. The name need not be UTF-8.
    ///
    /// The data should be written using the [`Write`] implementation on this [`ZipWriter`]
    pub fn start_file_with_options<T: FileOptionExtension>(
        &mut self,
        mut options: FileOptions<'_, '_, T>,
    ) -> ZipResult<()> {
        let Some(name) = options.name else {
            return Err(invalid!("File name must be specified"));
        };
        options.normalize();
        let make_new_self = self.inner.prepare_next_writer(
            options.compression_method,
            options.compression_level,
            #[cfg(feature = "deflate-zopfli")]
            options.zopfli_buffer_size,
        )?;
        self.start_entry(name, options, None)?;
        let result = self.switch_inner(make_new_self);
        self.ok_or_abort_file(result)?;
        self.writing_raw = false;
        Ok(())
    }

    #[doc(hidden)]
    pub fn add_raw_file<R: Read, T: FileOptionExtension>(
        &mut self,
        name: &[u8],
        options: FileOptions<'_, '_, T>,
        raw_values: ZipRawValues,
        mut content: R,
    ) -> ZipResult<()> {
        self.start_entry(name, options, Some(raw_values))?;
        self.writing_to_file = true;
        self.writing_raw = true;
        io::copy(&mut content, self)?;
        self.finish_file()
    }

    /// Add a directory entry.
    ///
    /// As directories have no content, you must not call [`ZipWriter::write`] before adding a new file.
    pub fn add_directory<S, T: FileOptionExtension>(
        &mut self,
        name: S,
        mut options: FileOptions<'_, '_, T>,
    ) -> ZipResult<()>
    where
        S: Into<String>,
    {
        let permissions = options
            .permissions
            .get_or_insert(FileOptions::DEFAULT_DIR_PERMISSIONS);
        *permissions |= ffi::S_IFDIR; // if permissions are already set by user
        options.compression_method = Stored;
        options.encrypt_with = None;

        let name_as_string = name.into();
        // Append a slash to the filename if it does not end with it.
        let name_with_slash = match name_as_string.chars().last() {
            Some('/' | '\\') => name_as_string,
            _ => name_as_string + "/",
        };

        self.start_entry(name_with_slash.as_bytes(), options, None)?;
        self.writing_to_file = false;
        self.switch_to_non_encrypting_writer()?;
        Ok(())
    }

    /// Finish the last file and write all other zip-structures
    ///
    /// This will return the writer, but one should normally not append any data to the end of the file.
    /// Note that the zipfile will also be finished on drop.
    pub fn finish(mut self) -> ZipResult<W> {
        let _central_start = self.finalize()?;
        self.close_writer()
    }

    fn close_writer(&mut self) -> ZipResult<W> {
        let inner = mem::replace(&mut self.inner, GenericZipWriter::Closed);
        match inner {
            GenericZipWriter::Storer(MaybeEncrypted::Unencrypted(w)) => Ok(w),
            _ => Err(invalid!(
                "Should have switched to stored and unencrypted beforehand",
            )),
        }
    }

    /// Add a symlink entry.
    ///
    /// The zip archive will contain an entry for path `name` which is a symlink to `target`.
    ///
    /// No validation or normalization of the paths is performed. For best results,
    /// callers should normalize `\` to `/` and ensure symlinks are relative to other
    /// paths within the zip archive.
    ///
    /// WARNING: not all zip implementations preserve symlinks on extract. Some zip
    /// implementations may materialize a symlink as a regular file, possibly with the
    /// content incorrectly set to the symlink target. For maximum portability, consider
    /// storing a regular file instead.
    pub fn add_symlink<N: ToString, T: ToString, E: FileOptionExtension>(
        &mut self,
        name: N,
        target: T,
        mut options: FileOptions<'_, '_, E>,
    ) -> ZipResult<()> {
        let permissions = options
            .permissions
            .get_or_insert(FileOptions::DEFAULT_SYMLINK_PERMISSIONS);
        *permissions |= ffi::S_IFLNK; // if permissions are already set by user
        // The symlink target is stored as file content. And compressing the target path
        // likely wastes space. So always store.
        options.compression_method = Stored;

        let file_name = name.to_string();
        self.start_entry(file_name.as_bytes(), options, None)?;
        self.writing_to_file = true;
        let result = self.write_all(target.to_string().as_bytes());
        self.ok_or_abort_file(result)?;
        self.writing_raw = false;
        self.finish_file()?;

        Ok(())
    }

    /// Add a file whose contents were already compressed by a [`ZipFileBuilder`].
    ///
    /// The compressed data is copied into the archive verbatim, so the compression work can be
    /// done ahead of time, possibly on another thread. See [`ZipFileBuilder`] for an example of
    /// parallelizing compression this way. The file must not have the same name as a file already
    /// in the archive.
    pub fn add_prepared_file(&mut self, file: PreparedZipFile) -> ZipResult<()> {
        let PreparedZipFile {
            file_name,
            options,
            crc32,
            uncompressed_size,
            compressed_size,
            data,
        } = file;
        let raw_values = ZipRawValues {
            crc32,
            compressed_size,
            uncompressed_size,
            language_encoding: None,
        };

        self.start_entry(&file_name, options, Some(raw_values))?;
        self.writing_raw = true;

        // start_entry leaves the inner writer as a bare Storer (a compression encoder is only
        // installed by start_file*), so the already-compressed bytes pass through unchanged.
        // Write directly to the inner writer rather than through ZipWriter::write, whose stats
        // bookkeeping (CRC-32 hashing and the large-file size check) is redundant here: the CRC
        // and sizes are already known, and large_file was set from them in finish().
        let result = self
            .inner
            .try_inner_mut()
            .and_then(|writer| writer.write_all(&data));
        self.ok_or_abort_file(result)?;

        // The local file header was written from the raw values, but for large files it contains
        // a ZIP64 extra field with placeholder sizes; go back and patch it now. When the
        // underlying writer doesn't support seeking, the entry uses a data descriptor instead,
        // which finish_file() skips for raw entries, so write it here.
        let auto_large_file = self.auto_large_file;
        let result = (|| -> ZipResult<()> {
            let writer = self.inner.try_inner_mut()?;
            let Some((file_name_raw, file)) = self.files.last_mut() else {
                debug_assert!(
                    false,
                    "Newly added file not found while finishing it in add_prepared_file"
                );
                return Err(ZipError::FileNotFound);
            };
            if file.is_using_data_descriptor() {
                file.write_data_descriptor(writer, auto_large_file)?;
            } else if file.large_file {
                let file_end = writer.stream_position()?;
                file.update_local_file_header(writer, file_name_raw)?;
                writer.seek(SeekFrom::Start(file_end))?;
            }
            Ok(())
        })();
        self.ok_or_abort_file(result)?;

        self.finish_file()
    }

    #[doc(hidden)]
    pub fn existing_file_names(&self) -> Vec<Box<[u8]>> {
        self.files.keys().cloned().collect()
    }

    /// Appends multiple pre-compressed [`PreparedZipFile`] entries into the archive.
    ///
    /// # Errors
    ///
    /// Returns an error if appending any prepared file fails.
    pub fn add_prepared_files(
        &mut self,
        files: impl IntoIterator<Item = PreparedZipFile>,
    ) -> ZipResult<()> {
        for file in files {
            self.add_prepared_file(file)?;
        }
        Ok(())
    }

    fn finalize(&mut self) -> ZipResult<u64> {
        /// Longest stale tail that [`Self::finalize`] blanks in place rather than moving the footer.
        const STALE_FILL_MAX_LEN: u64 = 0xFF00;
        self.finish_file()?;

        let (central_start, is_zip64) = self.write_central_and_footer()?;
        if self.seek_possible {
            let writer = self.inner.try_inner_mut()?;
            let footer_end = writer.stream_position()?;
            let stream_end = writer.seek(SeekFrom::End(0))?;
            let archive_end = stream_end - self.comment.len() as u64;
            if archive_end <= footer_end
                && footer_end < stream_end
                && stream_end - footer_end < STALE_FILL_MAX_LEN
            {
                // Stale bytes no longer than the comment are left after it (we can't truncate), but
                // blank them: they may hold an old footer that a reader would find instead of ours.
                // Use 0xFF, not 0: if the comment ends with part of an EOCD, a fill of zeros would
                // complete it into a valid empty EOCD that readers would pick instead of ours,
                // whereas 0xFF gives any such EOCD a comment length of at least 0xFF00, which can't
                // fit in fewer than STALE_FILL_MAX_LEN trailing bytes.
                writer.seek(SeekFrom::Start(footer_end))?;
                io::copy(&mut io::repeat(0xFF).take(stream_end - footer_end), writer)?;
            } else if footer_end < stream_end {
                // Data from an aborted file is past the end of the footer.

                // Overwrite the magic so the footer is no longer valid.
                writer.seek(SeekFrom::Start(central_start))?;
                writer.write_all(&0_u32.to_le_bytes())?;
                let start_zip32_cde = footer_end
                    - (size_of::<Magic>() + size_of::<Zip32CDEBlock>()) as u64
                    - self.comment.len() as u64;
                writer.seek(SeekFrom::Start(start_zip32_cde))?;
                writer.write_all(&0_u32.to_le_bytes())?;
                let zip64_extensible_len = self
                    .zip64_extensible_data_sector
                    .as_ref()
                    .map(|e| e.len() as u64)
                    .unwrap_or(0);
                if is_zip64 {
                    let start_zip64_locator = start_zip32_cde
                        - (size_of::<Magic>() + size_of::<Zip64CentralDirectoryEndLocator>())
                            as u64;
                    writer.seek(SeekFrom::Start(start_zip64_locator))?;
                    writer.write_all(&0_u32.to_le_bytes())?;
                    let start_zip64_cde = start_zip64_locator
                        - (size_of::<Magic>() + Zip64CentralDirectoryEnd::MIN_FULL_SIZE) as u64
                        - zip64_extensible_len;
                    writer.seek(SeekFrom::Start(start_zip64_cde))?;
                    writer.write_all(&0_u32.to_le_bytes())?;
                }

                // Rewrite the footer at the actual end.
                let central_and_footer_size = footer_end - central_start;
                writer.seek(SeekFrom::End(-(central_and_footer_size as i64)))?;
                #[cfg_attr(not(debug_assertions), allow(unused_variables))]
                let (_, new_is_zip64) = self.write_central_and_footer()?;
                #[cfg(debug_assertions)]
                {
                    let archive_end = if new_is_zip64 && new_is_zip64 != is_zip64 {
                        let new_size = (size_of::<Magic>()
                            + Zip64CentralDirectoryEnd::MIN_FULL_SIZE)
                            as u64
                            + zip64_extensible_len
                            + (size_of::<Magic>() + size_of::<Zip64CentralDirectoryEndLocator>())
                                as u64;
                        archive_end + new_size
                    } else {
                        archive_end
                    };
                    debug_assert!(
                        self.inner.try_inner_mut()?.stream_position()?
                            == archive_end + self.comment.len() as u64
                    );
                }
            } else {
                writer.seek(SeekFrom::Start(footer_end))?;
            }
        }

        Ok(central_start)
    }

    fn write_central_and_footer(&mut self) -> Result<(u64, bool), ZipError> {
        let writer = self.inner.try_inner_mut()?;

        let mut version_needed = u16::from(MIN_VERSION);
        let central_start = writer.stream_position()?;
        for (filename_raw, file) in &mut self.files {
            file.write_central_directory_header(writer, filename_raw)?;
            version_needed = version_needed.max(file.version_needed());
        }
        let central_size = writer.stream_position()? - central_start;
        let is64 = self.files.len() > ZIP64_ENTRY_THR
            || central_size.max(central_start) > ZIP64_BYTES_THR
            || self.zip64_extensible_data_sector.is_some();

        if is64 {
            let zip64_extensible_data_sector = self.zip64_extensible_data_sector.clone();
            let extensible_len = zip64_extensible_data_sector
                .as_ref()
                .map(|e| e.len() as u64)
                .unwrap_or(0);

            let zip64_footer = Zip64CentralDirectoryEnd {
                record_size: extensible_len + 44,
                version_made_by: version_needed,
                version_needed_to_extract: version_needed,
                disk_number: 0,
                disk_with_central_directory: 0,
                number_of_files_on_this_disk: self.files.len() as u64,
                number_of_files: self.files.len() as u64,
                central_directory_size: central_size,
                central_directory_offset: central_start,
                zip64_extensible_data_sector,
            };

            zip64_footer.write(writer)?;

            let zip64_footer = Zip64CentralDirectoryEndLocator {
                disk_with_central_directory: 0,
                end_of_central_directory_offset: central_start + central_size,
                number_of_disks: 1,
            };

            zip64_footer.write(writer)?;
        }

        let central_directory_size = if is64 {
            ZIP64_BYTES_THR_U32
        } else {
            central_size.min(ZIP64_BYTES_THR) as u32
        };

        let number_of_files = self.files.len().min(ZIP64_ENTRY_THR) as u16;
        let footer = Zip32CentralDirectoryEnd {
            disk_number: 0,
            disk_with_central_directory: 0,
            zip_file_comment: self.comment.clone(),
            number_of_files_on_this_disk: number_of_files,
            number_of_files,
            central_directory_size,
            central_directory_offset: central_start.min(ZIP64_BYTES_THR) as u32,
        };
        footer.write(writer)?;
        Ok((central_start, is64))
    }

    fn index_by_name(&self, name: &[u8]) -> ZipResult<usize> {
        self.files.get_index_of(name).ok_or(ZipError::FileNotFound)
    }

    /// Adds another entry to the central directory referring to the same content as an existing
    /// entry. The file's local-file header will still refer to it by its original name, so
    /// unzipping the file will technically be unspecified behavior. Some readers ignore the
    /// filename in the local-file header and treat the central directory as authoritative. However,
    /// some other software (e.g. Minecraft) will refuse to extract a file copied this way.
    pub fn shallow_copy_file(&mut self, src_name: &str, dest_name: &str) -> ZipResult<()> {
        self.finish_file()?;
        if src_name == dest_name {
            return Err(invalid!("Trying to copy a file to itself"));
        }
        let src_index = self.index_by_name(src_name.as_bytes())?;
        let mut dest_data = self.files[src_index].clone();
        let file_name_raw = dest_name.as_bytes();
        dest_data.central_header_start = 0;
        dest_data.set_language_encoding_for(file_name_raw);
        dest_data.drop_unicode_path_renaming(file_name_raw);
        self.insert_file_data(file_name_raw, dest_data)?;
        Ok(())
    }
}

#[cfg(feature = "rayon")]
#[inline]
fn default_data_channel_capacity() -> NonZeroUsize {
    NonZeroUsize::new(rayon::current_num_threads()).unwrap_or(NonZeroUsize::MIN)
}

#[cfg(feature = "rayon")]
impl<W: Write + Seek + Send> ZipWriter<W> {
    /// Compresses a collection of in-memory data entries in parallel using Rayon
    /// and streams them to the archive via a zero-mutex bounded channel pipeline.
    ///
    /// Compression occurs concurrently across available Rayon worker threads. Finished files
    /// are streamed into a bounded channel and written sequentially by the calling thread.
    /// Memory consumption is naturally bounded to `num_threads * file_size` via channel
    /// backpressure.
    ///
    /// Duplicate filenames (including CP437 raw byte overrides) are checked before compression
    /// begins to eliminate wasted CPU work.
    ///
    /// # Errors
    ///
    /// Returns an error if any entry fails compression, if duplicate filenames are detected,
    /// or if appending to the archive fails.
    pub fn par_append_data<'a, I, N, T, D>(&mut self, entries: I) -> ZipResult<()>
    where
        I: rayon::iter::IntoParallelIterator<Item = (N, FileOptions<'a, 'a, T>, D)>,
        N: AsRef<str> + Send,
        T: FileOptionExtension + Send + Sync + 'a,
        D: AsRef<[u8]> + Send,
    {
        self.par_append_data_with_capacity(entries, default_data_channel_capacity())
    }

    /// Compresses a collection of in-memory data entries in parallel using Rayon
    /// and streams them to the archive with a user-specified channel buffer capacity.
    ///
    /// `channel_capacity` bounds the maximum number of compressed files held in memory
    /// waiting for sequential I/O commit. Rayon worker threads automatically stall when
    /// the channel fills, providing hardware-level backpressure tailored to your storage
    /// throughput and memory budget.
    ///
    /// # Errors
    ///
    /// Returns an error if any entry fails compression, if duplicate filenames are detected,
    /// or if appending to the archive fails.
    pub fn par_append_data_with_capacity<'a, I, N, T, D>(
        &mut self,
        entries: I,
        channel_capacity: NonZeroUsize,
    ) -> ZipResult<()>
    where
        I: rayon::iter::IntoParallelIterator<Item = (N, FileOptions<'a, 'a, T>, D)>,
        N: AsRef<str> + Send,
        T: FileOptionExtension + Send + Sync + 'a,
        D: AsRef<[u8]> + Send,
    {
        use rayon::prelude::*;
        use std::collections::HashSet;
        use std::sync::Mutex;
        use std::sync::mpsc::sync_channel;

        let par_iter = entries.into_par_iter();
        let additional = par_iter.opt_len().unwrap_or(0);
        let mut initial_seen = HashSet::with_capacity(self.files.len().saturating_add(additional));
        for key in self.files.keys() {
            initial_seen.insert(key.clone());
        }
        let seen = Mutex::new(initial_seen);
        let (tx, rx) = sync_channel(channel_capacity.get());
        let mut write_err = None;

        std::thread::scope(|s| {
            let consumer_handle = s.spawn(|| -> ZipResult<()> {
                for item in rx {
                    self.add_prepared_file(item?)?;
                }
                Ok(())
            });

            let _ = par_iter.try_for_each(|(name, options, data)| -> Result<(), ()> {
                let name_str = name.as_ref();
                let name_bytes: Box<[u8]> = options.name.unwrap_or(name_str.as_bytes()).into();

                // Fast pre-check: verify uniqueness before starting expensive compression
                {
                    let mut seen_guard = match seen.lock() {
                        Ok(g) => g,
                        Err(_) => return Err(()),
                    };
                    if !seen_guard.insert(name_bytes) {
                        let err = invalid!("Duplicate filename: {:?}", name_str);
                        let _ = tx.send(Err(err));
                        return Err(());
                    }
                }

                // Compress concurrently on worker threads without holding any writer locks
                let prepared = match PreparedZipFile::from_data(name, options, data.as_ref()) {
                    Ok(p) => p,
                    Err(err) => {
                        let _ = tx.send(Err(err));
                        return Err(());
                    }
                };

                // Send to bounded channel; if receiver disconnected, abort immediately
                if tx.send(Ok(prepared)).is_err() {
                    return Err(());
                }

                Ok(())
            });

            drop(tx);

            if let Ok(res) = consumer_handle.join() {
                write_err = res.err();
            }
        });

        if let Some(err) = write_err {
            return Err(err);
        }
        Ok(())
    }

    /// Compresses a collection of in-memory data entries in parallel with atomic all-or-nothing guarantee.
    ///
    /// All entries are compressed into memory and validated before any files are appended to the archive.
    /// If any entry fails compression or if duplicate filenames are detected, no files are committed.
    ///
    /// # Errors
    ///
    /// Returns an error if any entry fails compression, if duplicates are found, or if writing fails.
    pub fn par_append_data_atomic<'a, I, N, T, D>(&mut self, entries: I) -> ZipResult<()>
    where
        I: rayon::iter::IntoParallelIterator<Item = (N, FileOptions<'a, 'a, T>, D)>,
        N: AsRef<str> + Send,
        T: FileOptionExtension + Send + Sync + 'a,
        D: AsRef<[u8]> + Send,
    {
        use rayon::prelude::*;
        let prepared_files: ZipResult<Vec<PreparedZipFile>> = entries
            .into_par_iter()
            .map(|(name, options, data)| PreparedZipFile::from_data(name, options, data.as_ref()))
            .collect();

        let prepared_files = prepared_files?;

        let mut seen = std::collections::HashSet::with_capacity(prepared_files.len());
        for file in &prepared_files {
            let name = file.file_name_raw();
            if self.files.contains_key(name) || !seen.insert(name) {
                return Err(invalid!("Duplicate filename: {:?}", name.escape_ascii()));
            }
        }

        for file in prepared_files {
            self.add_prepared_file(file)?;
        }
        Ok(())
    }
}

impl<W: Write> ZipWriter<StreamWriter<W>> {
    /// Creates a writer that doesn't require the inner writer to implement [Seek], but where
    /// operations that would overwrite previously-written bytes or cause subsequent operations to
    /// do so (such as `abort_file`) will always return an error.
    ///
    /// To use this writer from an async runtime, run it on a dedicated thread and forward its
    /// output through a bounded channel. See `examples/async_bridge.rs`.
    pub fn new_stream(inner: W) -> ZipWriter<StreamWriter<W>> {
        ZipWriter {
            inner: GenericZipWriter::Storer(MaybeEncrypted::Unencrypted(StreamWriter::new(inner))),
            files: IndexMap::new(),
            stats: ZipWriterStats::default(),
            writing_to_file: false,
            writing_raw: false,
            comment: Box::new([]),
            zip64_extensible_data_sector: None,
            flush_on_finish_file: false,
            seek_possible: false,
            auto_large_file: false,
            #[cfg(feature = "deflate-flate2")]
            deflate_cache: DeflateCache::default(),
        }
    }
}

impl<W: Write + Seek> Drop for ZipWriter<W> {
    fn drop(&mut self) {
        if !self.inner.is_closed()
            && let Err(e) = self.finalize()
        {
            let _ = write!(
                io::stderr(),
                "ZipWriter::drop: failed to finalize archive: {e:?}"
            );
        }
    }
}

/// Builds the next writer around the bare inner writer. The [`DeflateCache`] may hold an idle
/// DEFLATE encoder recycled from the previous entry.
type SwitchWriterFunction<W> =
    Box<dyn FnOnce(MaybeEncrypted<W>, &mut DeflateCache<W>) -> ZipResult<GenericZipWriter<W>>>;

/// Keeps the `flate2` DEFLATE encoder alive between entries, so that consecutive Deflated entries
/// at the same level reuse its compressor state (several hundred KiB) and output buffer instead of
/// reallocating them (#633).
///
/// While idle, the cached encoder wraps [`MaybeEncrypted::Closed`]:
/// - at the end of an entry, [`DeflateCache::park`] calls `DeflateEncoder::reset(Closed)`,
///   which finishes the entry's stream into the real writer, resets the `Compress`, clears the
///   encoder's output buffer and hands the real writer back;
/// - at the start of the next entry at the same level, [`DeflateCache::encoder`] swaps the real
///   writer back in through `DeflateEncoder::get_mut`. The compressor is already in its initial
///   state, so nothing is ever written to `Closed` in normal operation.
/// - if the cache is dropped while holding an idle encoder, flate2's `Drop` finishes the empty,
///   reset stream into `Closed`, which discards it.
#[cfg(feature = "deflate-flate2")]
pub(crate) struct DeflateCache<W: Write> {
    /// Level of the encoder most recently handed out. At most one `Deflater` is alive per cache,
    /// so this is also the level of the encoder handed back to [`DeflateCache::park`].
    level: u32,
    idle: Option<flate2::write::DeflateEncoder<MaybeEncrypted<W>>>,
}

#[cfg(feature = "deflate-flate2")]
impl<W: Write> Default for DeflateCache<W> {
    fn default() -> Self {
        Self {
            level: 0,
            idle: None,
        }
    }
}

#[cfg(feature = "deflate-flate2")]
impl<W: Write> Debug for DeflateCache<W> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match &self.idle {
            Some(_) => write!(f, "DeflateCache(level {})", self.level),
            None => f.write_str("DeflateCache(empty)"),
        }
    }
}

#[cfg(feature = "deflate-flate2")]
impl<W: Write> DeflateCache<W> {
    /// Returns an encoder at `level`, writing to `bare`, in the state of a fresh encoder.
    fn encoder(
        &mut self,
        bare: MaybeEncrypted<W>,
        level: u32,
    ) -> io::Result<flate2::write::DeflateEncoder<MaybeEncrypted<W>>> {
        let cached_level = mem::replace(&mut self.level, level);
        match self.idle.take() {
            Some(mut encoder) if cached_level == level => {
                // `park` already finished the previous stream and reset the compressor (and
                // cleared the encoder's output buffer), so only the writer needs swapping.
                // Using `reset(bare)` here instead would "finish" the empty stream into
                // `Closed` and reset the compressor a second time, which costs more than it
                // saves (see #1119).
                match mem::replace(encoder.get_mut(), bare) {
                    MaybeEncrypted::Closed { .. } => Ok(encoder),
                    other => {
                        // Unreachable by construction; don't lose the caller's writer.
                        let bare = mem::replace(encoder.get_mut(), other);
                        Ok(flate2::write::DeflateEncoder::new(
                            bare,
                            flate2::Compression::new(level),
                        ))
                    }
                }
            }
            _ => Ok(flate2::write::DeflateEncoder::new(
                bare,
                flate2::Compression::new(level),
            )),
        }
    }

    /// Finishes the entry's DEFLATE stream, returns the real writer and keeps the encoder.
    fn park(
        &mut self,
        mut encoder: flate2::write::DeflateEncoder<MaybeEncrypted<W>>,
    ) -> io::Result<MaybeEncrypted<W>> {
        let bare = encoder.reset(MaybeEncrypted::Closed { bytes_written: 0 })?;
        self.idle = Some(encoder);
        Ok(bare)
    }
}

/// Stand-in so that signatures don't depend on whether flate2 is enabled.
#[cfg(not(feature = "deflate-flate2"))]
pub(crate) struct DeflateCache<W>(core::marker::PhantomData<fn() -> W>);

#[cfg(not(feature = "deflate-flate2"))]
impl<W> Default for DeflateCache<W> {
    fn default() -> Self {
        Self(core::marker::PhantomData)
    }
}

#[cfg(not(feature = "deflate-flate2"))]
impl<W> Debug for DeflateCache<W> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.write_str("DeflateCache")
    }
}

impl<W: Write + Seek> GenericZipWriter<W> {
    fn prepare_next_writer(
        &self,
        compression: CompressionMethod,
        compression_level: Option<i64>,
        #[cfg(feature = "deflate-zopfli")] zopfli_buffer_size: Option<usize>,
    ) -> ZipResult<SwitchWriterFunction<W>> {
        if let GenericZipWriter::Closed = self {
            return Err(
                io::Error::new(io::ErrorKind::BrokenPipe, "ZipWriter was already closed").into(),
            );
        }

        match compression {
            Stored => {
                if compression_level.is_some() {
                    Err(UnsupportedArchive("Unsupported compression level"))
                } else {
                    Ok(Box::new(|bare, _| Ok(GenericZipWriter::Storer(bare))))
                }
            }
            #[allow(unreachable_code)]
            #[cfg(feature = "_deflate-any")]
            CompressionMethod::Deflated => {
                let default = {
                    #[cfg(feature = "deflate-flate2")]
                    {
                        i64::from(flate2::Compression::default().level())
                    }
                    #[cfg(all(not(feature = "deflate-flate2"), feature = "deflate-zopfli"))]
                    {
                        24
                    }
                    #[cfg(not(any(feature = "deflate-zopfli", feature = "deflate-flate2")))]
                    {
                        compile_error!("could not calculate default: unknown deflate variant")
                    }
                };

                let level = validate_value_in_range(
                    compression_level.unwrap_or(default),
                    deflate_compression_level_range(),
                )
                .ok_or(UnsupportedArchive("Unsupported compression level"))?;

                #[cfg(feature = "deflate-zopfli")]
                {
                    macro_rules! deflate_zopfli_and_return {
                        ($bare:expr, $best_non_zopfli:expr) => {{
                            let options = zopfli::Options {
                                iteration_count: core::num::NonZeroU64::try_from(u64::from(
                                    level - $best_non_zopfli,
                                ))
                                .map_err(|e| {
                                    std::io::Error::other(format!(
                                        "Failed to get the iteration count for zopfli : {e}"
                                    ))
                                })?,
                                ..Default::default()
                            };
                            Ok(Box::new(move |bare, _| {
                                Ok(match zopfli_buffer_size {
                                    Some(size) => GenericZipWriter::BufferedZopfliDeflater(
                                        std::io::BufWriter::with_capacity(
                                            size,
                                            zopfli::DeflateEncoder::new(
                                                options,
                                                Default::default(),
                                                bare,
                                            ),
                                        ),
                                    ),
                                    None => GenericZipWriter::ZopfliDeflater(
                                        zopfli::DeflateEncoder::new(
                                            options,
                                            Default::default(),
                                            bare,
                                        ),
                                    ),
                                })
                            }))
                        }};
                    }

                    #[cfg(feature = "deflate-flate2")]
                    {
                        let best_non_zopfli = flate2::Compression::best().level();
                        if level > best_non_zopfli {
                            return deflate_zopfli_and_return!(bare, best_non_zopfli);
                        }
                    }
                    #[cfg(not(feature = "deflate-flate2"))]
                    {
                        let best_non_zopfli = 9;
                        return deflate_zopfli_and_return!(bare, best_non_zopfli);
                    }
                }
                #[cfg(feature = "deflate-flate2")]
                {
                    Ok(Box::new(move |bare, cache: &mut DeflateCache<W>| {
                        Ok(GenericZipWriter::Deflater(cache.encoder(bare, level)?))
                    }))
                }
                #[cfg(not(feature = "deflate-flate2"))]
                {
                    unreachable!("deflate writer: have no fallback for this case")
                }
            }
            #[cfg(feature = "deflate64")]
            CompressionMethod::Deflate64 => {
                Err(UnsupportedArchive("Compressing Deflate64 is not supported"))
            }
            #[cfg(feature = "_bzip2_any")]
            CompressionMethod::Bzip2 => {
                let level: u32 = validate_value_in_range(
                    compression_level.unwrap_or(i64::from(bzip2::Compression::default().level())),
                    bzip2_compression_level_range(),
                )
                .ok_or(UnsupportedArchive("Unsupported compression level"))?;
                Ok(Box::new(move |bare, _| {
                    Ok(GenericZipWriter::Bzip2(bzip2::write::BzEncoder::new(
                        bare,
                        bzip2::Compression::new(level),
                    )))
                }))
            }
            CompressionMethod::AES => Err(UnsupportedArchive(
                "AES encryption is enabled through FileOptions::with_aes_encryption",
            )),
            #[cfg(feature = "zstd")]
            CompressionMethod::Zstd => {
                let level = validate_value_in_range(
                    compression_level.unwrap_or(i64::from(zstd::DEFAULT_COMPRESSION_LEVEL)),
                    zstd::compression_level_range(),
                )
                .ok_or(UnsupportedArchive("Unsupported compression level"))?;
                Ok(Box::new(move |bare, _| {
                    Ok(GenericZipWriter::Zstd(
                        zstd::stream::write::Encoder::new(bare, level as i32)
                            .map_err(ZipError::Io)?,
                    ))
                }))
            }
            #[cfg(feature = "legacy-zip")]
            CompressionMethod::Shrink => Err(ZipError::UnsupportedArchive(
                "Shrink compression unsupported",
            )),
            #[cfg(feature = "legacy-zip")]
            CompressionMethod::Reduce(_) => Err(ZipError::UnsupportedArchive(
                "Reduce compression unsupported",
            )),
            #[cfg(feature = "legacy-zip")]
            CompressionMethod::Implode => Err(ZipError::UnsupportedArchive(
                "Implode compression unsupported",
            )),
            #[cfg(feature = "lzma")]
            CompressionMethod::Lzma => {
                Err(UnsupportedArchive("LZMA isn't supported for compression"))
            }
            #[cfg(feature = "xz")]
            CompressionMethod::Xz => {
                let level = validate_value_in_range(compression_level.unwrap_or(6), 0_u32..=9_u32)
                    .ok_or(UnsupportedArchive("Unsupported compression level"))?;
                Ok(Box::new(move |bare, _| {
                    Ok(GenericZipWriter::Xz(Box::new(
                        lzma_rust2::XzWriter::new(bare, lzma_rust2::XzOptions::with_preset(level))
                            .map_err(ZipError::Io)?,
                    )))
                }))
            }
            #[cfg(feature = "ppmd")]
            CompressionMethod::Ppmd => {
                const ORDERS: [u32; 10] = [0, 4, 5, 6, 7, 8, 9, 10, 11, 12];

                let level = validate_value_in_range(compression_level.unwrap_or(7), 1_u32..=9_u32)
                    .ok_or(UnsupportedArchive("Unsupported compression level"))?;

                let order = ORDERS[level as usize];
                let memory_size = 1 << (level + 19);
                let memory_size_mb = memory_size / 1024 / 1024;

                Ok(Box::new(move |mut bare, _| {
                    let parameter: u16 = (order as u16 - 1)
                        + ((memory_size_mb - 1) << 4) as u16
                        + ((ppmd_rust::RestoreMethod::Restart as u16) << 12);

                    bare.write_all(&parameter.to_le_bytes())
                        .map_err(ZipError::Io)?;

                    let encoder = ppmd_rust::Ppmd8Encoder::new(
                        bare,
                        order,
                        memory_size,
                        ppmd_rust::RestoreMethod::Restart,
                    )
                    .map_err(|error| match error {
                        ppmd_rust::Error::RangeDecoderInitialization => ZipError::InvalidArchive(
                            "PPMd range coder initialization failed".into(),
                        ),
                        ppmd_rust::Error::InvalidParameter => {
                            ZipError::InvalidArchive("Invalid PPMd parameter".into())
                        }
                        ppmd_rust::Error::IoError(io_error) => ZipError::Io(io_error),
                        ppmd_rust::Error::MemoryAllocation => ZipError::Io(io::Error::new(
                            ErrorKind::OutOfMemory,
                            "PPMd could not allocate memory",
                        )),
                    })?;

                    Ok(GenericZipWriter::Ppmd(Box::new(encoder)))
                }))
            }
            #[allow(deprecated)]
            CompressionMethod::Unsupported(..) => {
                Err(UnsupportedArchive("Unsupported compression"))
            }
            _ => Err(UnsupportedArchive("Unsupported compression")),
        }
    }

    fn switch_to(
        &mut self,
        deflate_cache: &mut DeflateCache<W>,
        make_new_self: SwitchWriterFunction<W>,
    ) -> ZipResult<()> {
        let bare = match mem::replace(self, GenericZipWriter::Closed) {
            GenericZipWriter::Storer(w) => w,
            #[cfg(feature = "deflate-flate2")]
            GenericZipWriter::Deflater(w) => deflate_cache.park(w)?,
            #[cfg(feature = "deflate-zopfli")]
            GenericZipWriter::ZopfliDeflater(w) => w.finish()?,
            #[cfg(feature = "deflate-zopfli")]
            GenericZipWriter::BufferedZopfliDeflater(w) => w
                .into_inner()
                .map_err(|e| ZipError::Io(e.into_error()))?
                .finish()?,
            #[cfg(feature = "_bzip2_any")]
            GenericZipWriter::Bzip2(w) => w.finish()?,
            #[cfg(feature = "zstd")]
            GenericZipWriter::Zstd(w) => w.finish()?,
            #[cfg(feature = "xz")]
            GenericZipWriter::Xz(w) => w.finish()?,
            #[cfg(feature = "ppmd")]
            GenericZipWriter::Ppmd(w) => {
                // ZIP needs to encode an end marker (7z for example doesn't encode one).
                w.finish(true)?
            }
            GenericZipWriter::Closed => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "ZipWriter was already closed",
                )
                .into());
            }
        };
        *self = make_new_self(bare, deflate_cache)?;
        Ok(())
    }

    fn ref_mut(&mut self) -> Option<&mut dyn Write> {
        match self {
            GenericZipWriter::Storer(w) => Some(w as &mut dyn Write),
            #[cfg(feature = "deflate-flate2")]
            GenericZipWriter::Deflater(w) => Some(w as &mut dyn Write),
            #[cfg(feature = "deflate-zopfli")]
            GenericZipWriter::ZopfliDeflater(w) => Some(w as &mut dyn Write),
            #[cfg(feature = "deflate-zopfli")]
            GenericZipWriter::BufferedZopfliDeflater(w) => Some(w as &mut dyn Write),
            #[cfg(feature = "_bzip2_any")]
            GenericZipWriter::Bzip2(w) => Some(w as &mut dyn Write),
            #[cfg(feature = "zstd")]
            GenericZipWriter::Zstd(w) => Some(w as &mut dyn Write),
            #[cfg(feature = "xz")]
            GenericZipWriter::Xz(w) => Some(w as &mut dyn Write),
            #[cfg(feature = "ppmd")]
            GenericZipWriter::Ppmd(w) => Some(w as &mut dyn Write),
            GenericZipWriter::Closed => None,
        }
    }

    const fn is_closed(&self) -> bool {
        matches!(*self, GenericZipWriter::Closed)
    }

    fn try_inner_mut(&mut self) -> Result<&mut W, std::io::Error> {
        match *self {
            GenericZipWriter::Storer(MaybeEncrypted::Unencrypted(ref mut w)) => Ok(w),
            _ => Err(std::io::Error::other(
                "Should have switched to stored and unencrypted beforehand",
            )),
        }
    }
}

/// Compresses the contents of a single file entry independently of any [`ZipWriter`], so that
/// the CPU-intensive part of adding files to an archive can be parallelized.
///
/// The builder compresses the data written to it into memory as it arrives, while tracking the
/// metadata (CRC-32 and sizes) that the archive will need. Once [`ZipFileBuilder::finish`] has
/// been called, the resulting [`PreparedZipFile`] can be appended to an archive with
/// [`ZipWriter::add_prepared_file`], which copies the already-compressed bytes without
/// recompressing them.
///
/// Builders are self-contained: they can be created, written to and finished on other threads
/// while a `ZipWriter` is in use, and the prepared files appended to the archive as they become
/// ready.
///
/// Encryption is not supported; pass options without encryption or creation will fail.
///
/// ```
/// use std::io::{Cursor, Write};
/// use zip::ZipWriter;
/// use zip::write::{SimpleFileOptions, ZipFileBuilder};
///
/// # fn main() -> zip::result::ZipResult<()> {
/// let options = SimpleFileOptions::default();
///
/// // Compress each file on its own thread...
/// let handles = ["first.txt", "second.txt"].map(|name| {
///     std::thread::spawn(move || -> zip::result::ZipResult<_> {
///         let mut builder = ZipFileBuilder::new(name, options)?;
///         builder.write_all(b"Contents compressed off-thread")?;
///         builder.finish()
///     })
/// });
///
/// // ...then append them to the archive serially.
/// let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
/// for handle in handles {
///     zip.add_prepared_file(handle.join().unwrap()?)?;
/// }
/// let zip_bytes = zip.finish()?.into_inner();
/// # let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes))?;
/// # assert_eq!(archive.len(), 2);
/// # Ok(())
/// # }
/// ```
pub struct ZipFileBuilder {
    compressor: GenericZipWriter<Cursor<Vec<u8>>>,
    file_name: Box<[u8]>,
    options: FullFileOptions<'static, 'static>,
    hasher: Hasher,
    uncompressed_size: u64,
}

impl Debug for ZipFileBuilder {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!(
            "ZipFileBuilder {{file_name: {:?}, uncompressed_size: {}}}",
            self.file_name.escape_ascii().to_string(),
            self.uncompressed_size
        ))
    }
}

impl ZipFileBuilder {
    /// Creates a builder that compresses a file's contents using the compression settings from
    /// `options`.
    ///
    /// Fails if `options` requests encryption or an unsupported compression method or level.
    pub fn new<N: AsRef<str>, T: FileOptionExtension>(
        name: N,
        mut options: FileOptions<'_, '_, T>,
    ) -> ZipResult<Self> {
        if options.encrypt_with.is_some() {
            return Err(UnsupportedArchive(
                "Encrypted files cannot be compressed outside of a ZipWriter",
            ));
        }
        options.normalize();
        #[cfg(not(feature = "unreserved"))]
        {
            if let Some(extra_fields) = options.extended_options.extra_fields() {
                validate_extra_fields(extra_fields)?;
            }
        }
        let extended_options = ExtendedFileOptions {
            extra_fields: options
                .extended_options
                .extra_fields()
                .cloned()
                .unwrap_or_default(),
            file_comment: options.extended_options.take_file_comment(),
            validate_extra_fields: options.extended_options.validate_extra_fields(),
        };
        let owned_options = options.with_extended_options(extended_options);
        let mut compressor =
            GenericZipWriter::Storer(MaybeEncrypted::Unencrypted(Cursor::new(Vec::new())));
        let make_compressor = compressor.prepare_next_writer(
            owned_options.compression_method,
            owned_options.compression_level,
            #[cfg(feature = "deflate-zopfli")]
            owned_options.zopfli_buffer_size,
        )?;
        compressor.switch_to(&mut DeflateCache::default(), make_compressor)?;
        Ok(Self {
            compressor,
            file_name: name.as_ref().as_bytes().into(),
            options: owned_options,
            hasher: Hasher::new(),
            uncompressed_size: 0,
        })
    }

    /// Finishes compressing the file, and returns a [`PreparedZipFile`] that can be appended to
    /// an archive with [`ZipWriter::add_prepared_file`].
    pub fn finish(mut self) -> ZipResult<PreparedZipFile> {
        self.compressor.switch_to(
            &mut DeflateCache::default(),
            Box::new(|bare, _| Ok(GenericZipWriter::Storer(bare))),
        )?;
        let GenericZipWriter::Storer(MaybeEncrypted::Unencrypted(cursor)) = self.compressor else {
            return Err(ZipError::Io(io::Error::other(
                "Compressor was in an unexpected state",
            )));
        };
        let data = cursor.into_inner().into_boxed_slice();
        let compressed_size = data.len() as u64;
        let mut options = self.options;
        if compressed_size >= ZIP64_BYTES_THR || self.uncompressed_size >= ZIP64_BYTES_THR {
            options.large_file = true;
        }
        Ok(PreparedZipFile {
            file_name: self.file_name,
            options,
            crc32: self.hasher.finalize(),
            uncompressed_size: self.uncompressed_size,
            compressed_size,
            data,
        })
    }
}

impl Write for ZipFileBuilder {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let Some(w) = self.compressor.ref_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "write(): ZipFileBuilder was already closed",
            ));
        };
        let count = w.write(buf)?;
        self.hasher.update(&buf[..count]);
        self.uncompressed_size += count as u64;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.compressor.ref_mut() {
            Some(w) => w.flush(),
            None => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "flush(): ZipFileBuilder was already closed",
            )),
        }
    }
}

/// A file entry whose contents have already been compressed by a [`ZipFileBuilder`], ready to be
/// appended to an archive with [`ZipWriter::add_prepared_file`].
pub struct PreparedZipFile {
    file_name: Box<[u8]>,
    options: FullFileOptions<'static, 'static>,
    crc32: u32,
    uncompressed_size: u64,
    compressed_size: u64,
    data: Box<[u8]>,
}

impl Debug for PreparedZipFile {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!(
            "PreparedZipFile {{file_name: {:?}, crc32: {}, uncompressed_size: {}, compressed_size: {}}}",
            self.file_name.escape_ascii().to_string(),
            self.crc32,
            self.uncompressed_size,
            self.compressed_size
        ))
    }
}

impl PreparedZipFile {
    /// Compresses `data` in memory and prepares it as a [`PreparedZipFile`].
    ///
    /// # Errors
    ///
    /// Returns an error if compression fails or options are invalid.
    pub fn from_data<N: AsRef<str>, T: FileOptionExtension>(
        name: N,
        options: FileOptions<'_, '_, T>,
        data: &[u8],
    ) -> ZipResult<Self> {
        let mut builder = ZipFileBuilder::new(name, options)?;
        builder.write_all(data)?;
        builder.finish()
    }

    /// Returns the file name of this prepared file as a byte slice.
    #[must_use]
    pub fn file_name_raw(&self) -> &[u8] {
        &self.file_name
    }

    /// Returns the CRC-32 checksum of the uncompressed data.
    #[must_use]
    pub const fn crc32(&self) -> u32 {
        self.crc32
    }

    /// Returns the uncompressed size in bytes.
    #[must_use]
    pub const fn uncompressed_size(&self) -> u64 {
        self.uncompressed_size
    }

    /// Returns the compressed size in bytes.
    #[must_use]
    pub const fn compressed_size(&self) -> u64 {
        self.compressed_size
    }
}

#[cfg(feature = "_deflate-any")]
fn deflate_compression_level_range() -> std::ops::RangeInclusive<u32> {
    #[cfg(not(any(feature = "deflate-zopfli", feature = "deflate-flate2")))]
    {
        compile_error!("min: unknown deflate variant - enable deflate-zopfli or deflate-flate2")
    }

    let min = {
        #[cfg(feature = "deflate-flate2")]
        {
            flate2::Compression::fast().level()
        }
        #[cfg(all(not(feature = "deflate-flate2"), feature = "deflate-zopfli"))]
        {
            // Zopfli starts after the best flate2 level, 9
            10
        }
    };

    let max = {
        #[cfg(feature = "deflate-zopfli")]
        {
            264
        }
        #[cfg(all(not(feature = "deflate-zopfli"), feature = "deflate-flate2"))]
        {
            flate2::Compression::best().level()
        }
    };

    min..=max
}

#[cfg(feature = "_bzip2_any")]
fn bzip2_compression_level_range() -> std::ops::RangeInclusive<u32> {
    let min = bzip2::Compression::fast().level();
    let max = bzip2::Compression::best().level();
    min..=max
}

#[cfg(any(
    feature = "_deflate-any",
    feature = "_bzip2_any",
    feature = "ppmd",
    feature = "xz",
    feature = "zstd",
))]
fn validate_value_in_range<T: Ord + Copy, U: Ord + Copy + TryFrom<T>>(
    value: T,
    range: std::ops::RangeInclusive<U>,
) -> Option<U> {
    let value: U = value.try_into().ok()?;
    if range.contains(&value) {
        Some(value)
    } else {
        None
    }
}

#[cfg(feature = "aes-crypto")]
fn update_aes_extra_field<W: Write + Seek>(
    writer: &mut W,
    file: &mut ZipFileData,
    bytes_written: u64,
) -> ZipResult<()> {
    use zip_core_lib::extra_fields::AexEncryption;
    use zip_core_lib::format::aes::AesVendorVersion;

    let inner_compression_method = file.compression_method;

    // We prefer using AE-1 which provides an extra CRC check, but for small files we
    // switch to AE-2 to prevent being able to use the CRC value to to reconstruct the
    // unencrypted contents.
    //
    // We can only do this when the underlying writer supports
    // seek operations, so we gate this behind using_data_descriptor.
    //
    // C.f. https://www.winzip.com/en/support/aes-encryption/#crc-faq
    let new_version = if bytes_written < 20 {
        AesVendorVersion::Ae2
    } else {
        AesVendorVersion::Ae1
    };

    // edit the extra field
    if let Some(ExtraField::AeXEncryption(AexEncryption {
        aes_mode,
        aes_vendor_version,
        aes_extra_field_start,
        ..
    })) = &mut file
        .extra_fields
        .inner
        .iter_mut()
        .find(|f| matches!(f, ExtraField::AeXEncryption(_)))
    {
        *aes_vendor_version = new_version;

        if let Some(aes_start) = aes_extra_field_start {
            writer.seek(SeekFrom::Start(*aes_start as u64))?;
        } else {
            return Err(invalid!("The AES extra field should have a known start"));
        }

        let aes_extra_field =
            AexEncryption::new(*aes_vendor_version, *aes_mode, inner_compression_method);
        let mut buf = [0u8; AexEncryption::FULL_SIZE];
        aes_extra_field.write(&mut buf.as_mut_slice())?;
        writer.write_all(&buf)?;
    }

    Ok(())
}

/// Wrapper around a [Write] implementation that implements the [Seek] trait, but where seeking
/// returns an error unless it's a no-op.
pub struct StreamWriter<W: Write> {
    inner: W,
    bytes_written: u64,
}

impl<W: Write> StreamWriter<W> {
    /// Creates an instance wrapping the provided inner writer.
    pub fn new(inner: W) -> StreamWriter<W> {
        Self {
            inner,
            bytes_written: 0,
        }
    }

    /// Gets a reference to the underlying writer.
    pub fn get_ref(&self) -> &W {
        &self.inner
    }

    /// Gets a mutable reference to the underlying writer.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    /// Consumes this wrapper, returning the underlying writer.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for StreamWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let bytes_written = self.inner.write(buf)?;
        self.bytes_written += bytes_written as u64;
        Ok(bytes_written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write> Seek for StreamWriter<W> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Current(0) | SeekFrom::End(0) => return Ok(self.bytes_written),
            SeekFrom::Start(x) if x == self.bytes_written => {
                return Ok(self.bytes_written);
            }
            _ => {}
        }
        Err(io::Error::new(
            ErrorKind::Unsupported,
            "seek is not supported",
        ))
    }
}
