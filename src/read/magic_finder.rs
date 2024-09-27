use std::io::{Read, Seek, SeekFrom};

use memchr::memmem::FinderRev;

use crate::result::ZipResult;

/// A utility for finding magic symbols from the end of a seekable reader.
///
/// Can be repurposed to recycle the internal buffer.
pub struct MagicFinder<'a> {
    buffer: Box<[u8]>,
    pub(self) finder: FinderRev<'a>,
    cursor: u64,
    mid_buffer_offset: Option<usize>,
    bounds: (u64, u64),
}

impl<'a> MagicFinder<'a> {
    /// Create a new magic bytes finder to look within specific bounds.
    pub fn new(magic_bytes: &'a [u8], bounds: (u64, u64)) -> Self {
        const BUFFER_SIZE: usize = 2048;

        // Smaller buffer size would be unable to locate bytes.
        // Equal buffer size would stall (the window could not be moved).
        debug_assert!(BUFFER_SIZE > magic_bytes.len());

        Self {
            buffer: vec![0; BUFFER_SIZE].into_boxed_slice(),
            finder: FinderRev::new(magic_bytes),
            cursor: bounds
                .1
                .saturating_sub(BUFFER_SIZE as u64)
                .clamp(bounds.0, bounds.1),
            mid_buffer_offset: None,
            bounds,
        }
    }

    /// Repurpose the finder for different bytes or bounds.
    pub fn repurpose(&mut self, magic_bytes: &'a [u8], bounds: (u64, u64)) -> &mut Self {
        debug_assert!(self.buffer.len() > magic_bytes.len());

        self.finder = FinderRev::new(magic_bytes);
        self.cursor = bounds
            .1
            .saturating_sub(self.buffer.len() as u64)
            .clamp(bounds.0, bounds.1);
        self.bounds = bounds;

        // Reset the mid-buffer offset, to invalidate buffer content.
        self.mid_buffer_offset = None;

        self
    }

    /// Find the next magic bytes from the end of the reader.
    ///
    /// Similar in functionality to a double ended iterator, except
    /// it propagates errors first and doesn't hold on to the reader
    /// between items.
    pub fn next_back<R: Read + Seek>(&mut self, reader: &mut R) -> ZipResult<Option<u64>> {
        loop {
            if self.cursor < self.bounds.0 {
                // The finder is consumed
                break;
            }

            /* Position the window and ensure correct length */
            let window_start = self.cursor;
            let window_end = self
                .cursor
                .saturating_add(self.buffer.len() as u64)
                .min(self.bounds.1);

            if window_end <= window_start {
                // Short-circuit on zero-sized windows to prevent loop
                break;
            }

            let window = &mut self.buffer[..(window_end - window_start) as usize];

            if self.mid_buffer_offset.is_none() {
                reader.seek(SeekFrom::Start(window_start))?;
                reader.read_exact(window)?;
            }

            let mid_buffer_offset = self.mid_buffer_offset.unwrap_or(window.len());
            let window = &mut window[..mid_buffer_offset];

            if let Some(offset) = self.finder.rfind(window) {
                let magic_pos = window_start + offset as u64;
                reader.seek(SeekFrom::Start(magic_pos))?;

                self.mid_buffer_offset = Some(offset);

                return Ok(Some(magic_pos));
            }

            self.mid_buffer_offset = None;

            /* We always want to make sure we go allllll the way back to the start of the file if
             * we can't find it elsewhere. However, our `while` condition doesn't check that. So we
             * avoid infinite looping by checking at the end of the loop. */
            if window_start == self.bounds.0 {
                self.bounds.0 = self.bounds.1;
                break;
            }

            /* Move cursor to the next chunk, cover magic at boundary by shifting by needle length. */
            self.cursor = self
                .cursor
                .saturating_add(self.finder.needle().len() as u64)
                .saturating_sub(self.buffer.len() as u64)
                .clamp(self.bounds.0, self.bounds.1);
        }

        Ok(None)
    }
}

/// A magic bytes finder with an optimistic guess that is tried before
/// the inner finder begins searching from end. This enables much faster
/// lookup in files without appended junk, because the magic bytes will be
/// found directly.
///
/// The guess can be marked as mandatory to produce an error. This is useful
/// if the ArchiveOffset is known and auto-detection is not desired.
pub struct OptimisticMagicFinder<'a> {
    inner: MagicFinder<'a>,
    initial_guess: Option<(u64, bool)>,
}

/// This is a temporary restriction, to avoid heap allocation in [`Self::next_back`].
///
/// We only use magic bytes of size 4 at the moment.
const STACK_BUFFER_SIZE: usize = 8;

impl<'a> OptimisticMagicFinder<'a> {
    /// Create a new empty optimistic magic bytes finder.
    pub fn new_empty() -> Self {
        Self {
            inner: MagicFinder::new(&[], (0, 0)),
            initial_guess: None,
        }
    }

    /// Repurpose the finder for different bytes, bounds and initial guesses.
    pub fn repurpose(
        &mut self,
        magic_bytes: &'a [u8],
        bounds: (u64, u64),
        initial_guess: Option<(u64, bool)>,
    ) -> &mut Self {
        debug_assert!(magic_bytes.len() <= STACK_BUFFER_SIZE);

        self.inner.repurpose(magic_bytes, bounds);
        self.initial_guess = initial_guess;

        self
    }

    /// Equivalent to `next_back`, with an optional initial guess attempted before
    /// proceeding with reading from the back of the file.
    pub fn next_back<R: Read + Seek>(&mut self, reader: &mut R) -> ZipResult<Option<u64>> {
        if let Some((v, mandatory)) = self.initial_guess {
            reader.seek(SeekFrom::Start(v))?;

            let mut buffer = [0; STACK_BUFFER_SIZE];
            let buffer = &mut buffer[..self.inner.finder.needle().len()];

            // Attempt to match only if there's enough space for the needle
            if v.saturating_add(buffer.len() as u64) <= self.inner.bounds.1 {
                reader.read_exact(buffer)?;

                // If a match is found, yield it.
                if self.inner.finder.rfind(&buffer).is_some() {
                    self.initial_guess.take();
                    reader.seek(SeekFrom::Start(v))?;
                    return Ok(Some(v));
                }
            }

            // If a match is not found, but the initial guess was mandatory, return an error.
            if mandatory {
                return Ok(None);
            }
        }

        self.inner.next_back(reader)
    }
}
