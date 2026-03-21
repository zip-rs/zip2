//! Code related to reader

use std::io::{self, Read, Seek, SeekFrom};

pub(crate) enum ZipFileSeekReader<'a, R: ?Sized> {
    Raw(SeekableTake<'a, R>),
}

pub(crate) struct SeekableTake<'a, R: ?Sized> {
    inner: &'a mut R,
    inner_starting_offset: u64,
    length: u64,
    current_offset: u64,
}

impl<'a, R: Seek + ?Sized> SeekableTake<'a, R> {
    pub fn new(inner: &'a mut R, length: u64) -> io::Result<Self> {
        let inner_starting_offset = inner.stream_position()?;
        Ok(Self {
            inner,
            inner_starting_offset,
            length,
            current_offset: 0,
        })
    }
}

impl<R: Seek + ?Sized> Seek for SeekableTake<'_, R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let offset = match pos {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(offset) => self.length.checked_add_signed(offset),
            SeekFrom::Current(offset) => self.current_offset.checked_add_signed(offset),
        };
        match offset {
            None => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid seek to a negative or overflowing position",
            )),
            Some(offset) => {
                let clamped_offset = std::cmp::min(self.length, offset);
                let new_inner_offset = self
                    .inner
                    .seek(SeekFrom::Start(self.inner_starting_offset + clamped_offset))?;
                self.current_offset = new_inner_offset - self.inner_starting_offset;
                Ok(self.current_offset)
            }
        }
    }
}

impl<R: Read + ?Sized> Read for SeekableTake<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let written = self
            .inner
            .take(self.length - self.current_offset)
            .read(buf)?;
        self.current_offset += written as u64;
        Ok(written)
    }
}
