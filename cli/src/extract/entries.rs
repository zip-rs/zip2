use std::{fs, io, ops};

use zip::{
    read::{read_zipfile_from_stream, ZipFile},
    ZipArchive,
};

use crate::{CommandError, WrapCommandErr};

pub trait IterateEntries {
    fn next_entry(&mut self) -> Result<Option<ZipFile>, CommandError>;
}

pub struct ReadChecker<R> {
    inner: R,
    bytes_read: u64,
}

impl<R> ReadChecker<R> {
    pub const fn current_bytes_read(&self) -> u64 {
        self.bytes_read
    }
}

impl<R> ReadChecker<R>
where
    R: io::Read,
{
    pub fn exhaust(mut self) -> io::Result<(R, u64)> {
        io::copy(&mut self, &mut io::sink())?;
        let Self { inner, bytes_read } = self;
        Ok((inner, bytes_read))
    }
}

impl<R> io::Read for ReadChecker<R>
where
    R: io::Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        let num_read: u64 = n.try_into().unwrap();
        self.bytes_read += num_read;
        Ok(n)
    }
}

pub struct StreamInput<R> {
    inner: ReadChecker<R>,
    entries_read: usize,
}

impl<R> StreamInput<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner: ReadChecker {
                inner,
                bytes_read: 0,
            },
            entries_read: 0,
        }
    }

    pub fn into_inner(self) -> (ReadChecker<R>, usize) {
        let Self {
            inner,
            entries_read,
        } = self;
        (inner, entries_read)
    }
}

impl<R> IterateEntries for StreamInput<R>
where
    R: io::Read,
{
    fn next_entry(&mut self) -> Result<Option<ZipFile>, CommandError> {
        if let Some(entry) = read_zipfile_from_stream(&mut self.inner)
            .wrap_err("failed to read zip entries from stdin")?
        {
            self.entries_read += 1;
            Ok(Some(entry))
        } else {
            Ok(None)
        }
    }
}

#[derive(Debug)]
pub struct ZipFileInput<A> {
    inner: A,
    file_counter: usize,
}

impl<A> ZipFileInput<A> {
    pub fn new(inner: A) -> Self {
        Self {
            inner,
            file_counter: 0,
        }
    }
}

impl<A> ZipFileInput<A>
where
    A: ops::Deref<Target = ZipArchive<fs::File>>,
{
    pub fn remaining(&self) -> usize {
        self.inner.len() - self.file_counter
    }

    pub fn none_left(&self) -> bool {
        self.remaining() == 0
    }
}

impl<A> IterateEntries for ZipFileInput<A>
where
    A: ops::DerefMut<Target = ZipArchive<fs::File>>,
{
    fn next_entry(&mut self) -> Result<Option<ZipFile>, CommandError> {
        if self.none_left() {
            return Ok(None);
        }
        let prev_counter = self.file_counter;
        self.file_counter += 1;
        self.inner
            .by_index(prev_counter)
            .map(Some)
            .wrap_err_with(|| format!("failed to read entry #{prev_counter} from zip",))
    }
}
