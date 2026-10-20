//! A DEFLATE writer whose compressor state can be recycled across ZIP entries.
//!
//! `flate2::write::DeflateEncoder` owns its `flate2::Compress`, and the only public way to keep
//! that state alive past the end of a stream is `DeflateEncoder::reset`, which needs a
//! replacement writer of the same type. `ZipWriter` has no spare `MaybeEncrypted<W>` to hand it,
//! so every entry used to drop the encoder and build a new one (several hundred KiB of heap for
//! zlib-rs). This module drives `flate2::Compress` directly, using the same loops as
//! `flate2::zio::Writer`, so that the compressor and output buffer can be returned to the caller
//! once a stream is finished, then `reset()` and reused for the next entry.
//!
//! The write/flush/finish loops below mirror `flate2::zio::Writer` (flate2 1.1) call for call, so
//! the compressed output is byte-identical to `DeflateEncoder` for the same input, write
//! boundaries and flushes.

use flate2::{Compress, Compression, FlushCompress, Status};
use std::io::{self, Write};

/// Size of the intermediate output buffer, identical to `flate2::zio::Writer`.
const BUF_CAPACITY: usize = 32 * 1024;

/// Compressor state that survives between entries.
pub(crate) struct DeflateState {
    level: u32,
    compress: Compress,
    buf: Vec<u8>,
}

impl DeflateState {
    fn new(level: u32) -> Self {
        Self {
            level,
            compress: Compress::new(Compression::new(level), false),
            buf: Vec::with_capacity(BUF_CAPACITY),
        }
    }
}

/// Holds at most one idle [`DeflateState`] for reuse by the next DEFLATE entry.
#[derive(Default)]
pub(crate) struct DeflateCache(Option<DeflateState>);

impl DeflateCache {
    /// Returns a compressor at `level` in its initial state, reusing the cached one when the
    /// level matches. On a level mismatch the cached compressor is dropped and a fresh one is
    /// built, rather than calling `Compress::set_level`, which is only available on some flate2
    /// backends and whose effect on a reset stream is backend-specific.
    pub(crate) fn take(&mut self, level: u32) -> DeflateState {
        match self.0.take() {
            Some(mut state) if state.level == level => {
                state.compress.reset();
                state.buf.clear();
                state
            }
            Some(DeflateState { mut buf, .. }) => {
                // Keep the already-grown output buffer; its contents are irrelevant.
                buf.clear();
                DeflateState {
                    level,
                    compress: Compress::new(Compression::new(level), false),
                    buf,
                }
            }
            None => DeflateState::new(level),
        }
    }

    pub(crate) fn put(&mut self, state: DeflateState) {
        self.0 = Some(state);
    }
}

impl core::fmt::Debug for DeflateCache {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self.0 {
            Some(s) => write!(f, "DeflateCache(level {})", s.level),
            None => f.write_str("DeflateCache(empty)"),
        }
    }
}

fn missing_writer() -> io::Error {
    io::Error::other("DEFLATE writer used after it was finished")
}

fn corrupt() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "corrupt deflate stream")
}

/// Raw-DEFLATE `Write` adapter; behaves like `flate2::write::DeflateEncoder` but can give back
/// its [`DeflateState`].
pub(crate) struct ReusableDeflateEncoder<W: Write> {
    /// `None` only after `finish` has moved the contents out.
    inner: Option<Inner<W>>,
}

struct Inner<W> {
    obj: W,
    state: DeflateState,
}

impl<W: Write> Inner<W> {
    fn dump(&mut self) -> io::Result<()> {
        while !self.state.buf.is_empty() {
            let n = self.obj.write(&self.state.buf)?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            self.state.buf.drain(..n);
        }
        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        loop {
            self.dump()?;
            let before = self.state.compress.total_out();
            self.state
                .compress
                .compress_vec(&[], &mut self.state.buf, FlushCompress::Finish)
                .map_err(io::Error::from)?;
            if before == self.state.compress.total_out() {
                return Ok(());
            }
        }
    }
}

impl<W: Write> ReusableDeflateEncoder<W> {
    /// `state` must be freshly constructed or obtained from [`DeflateCache::take`].
    pub(crate) fn new(obj: W, state: DeflateState) -> Self {
        Self {
            inner: Some(Inner { obj, state }),
        }
    }

    pub(crate) fn get_ref(&self) -> Option<&W> {
        self.inner.as_ref().map(|i| &i.obj)
    }

    pub(crate) fn get_mut(&mut self) -> Option<&mut W> {
        self.inner.as_mut().map(|i| &mut i.obj)
    }

    fn inner(&mut self) -> io::Result<&mut Inner<W>> {
        self.inner.as_mut().ok_or_else(missing_writer)
    }

    /// Terminates the DEFLATE stream and returns the inner writer together with the compressor
    /// state for reuse. On error the stream is left as-is (and `Drop` will retry finishing it,
    /// exactly like `flate2::zio::Writer`).
    pub(crate) fn finish(mut self) -> io::Result<(W, DeflateState)> {
        self.inner()?.finish()?;
        let Inner { obj, state } = self.inner.take().ok_or_else(missing_writer)?;
        Ok((obj, state))
    }
}

impl<W: Write> Write for ReusableDeflateEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let inner = self.inner()?;
        loop {
            inner.dump()?;
            let before_in = inner.state.compress.total_in();
            let ret =
                inner
                    .state
                    .compress
                    .compress_vec(buf, &mut inner.state.buf, FlushCompress::None);
            let written = (inner.state.compress.total_in() - before_in) as usize;
            let is_stream_end = matches!(ret, Ok(Status::StreamEnd));
            if !buf.is_empty() && written == 0 && ret.is_ok() && !is_stream_end {
                continue;
            }
            return match ret {
                Ok(_) => Ok(written),
                Err(_) => Err(corrupt()),
            };
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let inner = self.inner()?;
        inner
            .state
            .compress
            .compress_vec(&[], &mut inner.state.buf, FlushCompress::Sync)
            .map_err(io::Error::from)?;
        loop {
            inner.dump()?;
            let before = inner.state.compress.total_out();
            inner
                .state
                .compress
                .compress_vec(&[], &mut inner.state.buf, FlushCompress::None)
                .map_err(io::Error::from)?;
            if before == inner.state.compress.total_out() {
                break;
            }
        }
        inner.obj.flush()
    }
}

impl<W: Write> Drop for ReusableDeflateEncoder<W> {
    fn drop(&mut self) {
        // Same best-effort behaviour as `flate2::zio::Writer`'s `Drop`.
        if let Some(inner) = self.inner.as_mut() {
            let _ = inner.finish();
        }
    }
}
