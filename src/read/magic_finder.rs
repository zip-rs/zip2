use std::io::{Read, Seek, SeekFrom};

use memchr::memmem::FinderRev;

use crate::result::ZipResult;

pub struct MagicFinder<'a> {
    buffer: Box<[u8]>,
    pub(self) finder: FinderRev<'a>,
    cursor: u64,
    mid_buffer_offset: Option<usize>,
    bounds: (u64, u64),
}

const BUFFER_SIZE: usize = 2048;

impl<'a> MagicFinder<'a> {
    pub fn new(magic_bytes: &'a [u8], bounds: (u64, u64)) -> Self {
        debug_assert!(BUFFER_SIZE > magic_bytes.len());

        Self {
            buffer: vec![0; BUFFER_SIZE].into_boxed_slice(),
            finder: FinderRev::new(magic_bytes),
            cursor: bounds.1.saturating_sub(BUFFER_SIZE as u64).max(bounds.0),
            mid_buffer_offset: None,
            bounds,
        }
    }

    pub fn repurpose(&mut self, magic_bytes: &'a [u8], bounds: (u64, u64)) -> &mut Self {
        debug_assert!(BUFFER_SIZE > magic_bytes.len());

        self.finder = FinderRev::new(magic_bytes);
        self.cursor = bounds.1.saturating_sub(BUFFER_SIZE as u64).max(bounds.0);
        self.mid_buffer_offset = None;
        self.bounds = bounds;

        self
    }

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
                .saturating_add(BUFFER_SIZE as u64)
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

pub struct OptimisticMagicFinder<'a> {
    inner: MagicFinder<'a>,
    optimistic_offset: Option<(u64, bool)>,
}
impl<'a> OptimisticMagicFinder<'a> {
    pub fn new_empty() -> Self {
        Self {
            inner: MagicFinder::new(&[], (0, 0)),
            optimistic_offset: None,
        }
    }

    pub fn repurpose(
        &mut self,
        magic_bytes: &'a [u8],
        bounds: (u64, u64),
        optimistic_offset: Option<(u64, bool)>,
    ) -> &mut Self {
        self.inner.repurpose(magic_bytes, bounds);
        self.optimistic_offset = optimistic_offset;

        self
    }

    pub fn next_back<R: Read + Seek>(&mut self, reader: &mut R) -> ZipResult<Option<u64>> {
        if let Some((v, mandatory)) = self.optimistic_offset.take() {
            reader.seek(SeekFrom::Start(v))?;

            /* FIXME: remove the heap allocation */
            let mut buffer = vec![0u8; self.inner.finder.needle().len()];
            reader.read_exact(&mut buffer)?;

            /* If matches, rewind and return */
            if self.inner.finder.rfind(&buffer).is_some() {
                reader.seek(SeekFrom::Start(v))?;
                return Ok(Some(v));
            }

            if mandatory {
                return Ok(None);
            }
        }

        self.inner.next_back(reader)
    }
}
