use std::{cell::UnsafeCell, collections::VecDeque, fs, io, ops, path::Path};

use zip::{
    read::{read_zipfile_from_stream, ZipFile},
    ZipArchive,
};

use crate::{args::extract::*, CommandError, WrapCommandErr};

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

impl StreamInput<io::Stdin> {
    pub fn stdin() -> Self {
        Self::new(io::stdin())
    }
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

pub struct AllInputZips {
    zips_todo: VecDeque<ZipFileInput<Box<ZipArchive<fs::File>>>>,
    cur_zip: UnsafeCell<ZipFileInput<Box<ZipArchive<fs::File>>>>,
}

impl AllInputZips {
    pub fn new(
        zip_paths: impl IntoIterator<Item = impl AsRef<Path>>,
    ) -> Result<Self, CommandError> {
        let mut zips_todo = zip_paths
            .into_iter()
            .map(|p| {
                fs::File::open(p.as_ref())
                    .wrap_err_with(|| {
                        format!("failed to open zip input file path {:?}", p.as_ref())
                    })
                    .and_then(|f| {
                        ZipArchive::new(f).wrap_err_with(|| {
                            format!("failed to create zip archive for file {:?}", p.as_ref())
                        })
                    })
                    .map(|zip| ZipFileInput::new(Box::new(zip)))
            })
            .collect::<Result<VecDeque<_>, CommandError>>()?;
        debug_assert!(!zips_todo.is_empty());
        let cur_zip = zips_todo.pop_front().unwrap();
        Ok(Self {
            zips_todo,
            cur_zip: UnsafeCell::new(cur_zip),
        })
    }
}

impl IterateEntries for AllInputZips {
    fn next_entry(&mut self) -> Result<Option<ZipFile>, CommandError> {
        loop {
            if let Some(entry) = unsafe { &mut *self.cur_zip.get() }.next_entry()? {
                return Ok(Some(entry));
            }
            match self.zips_todo.pop_front() {
                Some(zip) => {
                    self.cur_zip = UnsafeCell::new(zip);
                }
                None => {
                    return Ok(None);
                }
            }
        }
    }
}

pub struct MergedInput {
    stdin_stream: Option<UnsafeCell<StreamInput<io::Stdin>>>,
    zips: Option<AllInputZips>,
}

impl MergedInput {
    pub fn from_spec(spec: InputSpec) -> Result<Self, CommandError> {
        let InputSpec {
            stdin_stream,
            zip_paths,
        } = spec;
        Ok(Self {
            stdin_stream: if stdin_stream {
                Some(UnsafeCell::new(StreamInput::stdin()))
            } else {
                None
            },
            zips: if zip_paths.is_empty() {
                None
            } else {
                Some(AllInputZips::new(zip_paths)?)
            },
        })
    }
}

impl IterateEntries for MergedInput {
    fn next_entry(&mut self) -> Result<Option<ZipFile>, CommandError> {
        let mut completed_stdin: bool = false;
        if let Some(stdin_stream) = self.stdin_stream.as_mut() {
            if let Some(entry) = unsafe { &mut *stdin_stream.get() }.next_entry()? {
                return Ok(Some(entry));
            }
            completed_stdin = true;
        }
        if completed_stdin {
            self.stdin_stream = None;
        }
        if let Some(zips) = self.zips.as_mut() {
            if let Some(entry) = zips.next_entry()? {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }
}
