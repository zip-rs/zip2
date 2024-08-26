use std::{
    cell::UnsafeCell,
    collections::VecDeque,
    fs,
    io::{self},
    path::Path,
};

use zip::{
    read::{read_zipfile_from_stream, ZipFile},
    ZipArchive,
};

use crate::{args::extract::*, CommandError, WrapCommandErr};

pub trait IterateEntries {
    fn next_entry(&mut self) -> Result<Option<ZipFile>, CommandError>;
}

struct StdinInput {
    inner: io::Stdin,
}

impl StdinInput {
    pub fn new() -> Self {
        Self { inner: io::stdin() }
    }
}

impl IterateEntries for StdinInput {
    fn next_entry(&mut self) -> Result<Option<ZipFile>, CommandError> {
        read_zipfile_from_stream(&mut self.inner).wrap_err("failed to read zip entries from stdin")
    }
}

#[derive(Debug)]
struct ZipFileInput {
    inner: ZipArchive<fs::File>,
    file_counter: usize,
}

impl ZipFileInput {
    pub fn new(inner: ZipArchive<fs::File>) -> Self {
        Self {
            inner: inner,
            file_counter: 0,
        }
    }

    pub fn remaining(&self) -> usize {
        self.inner.len() - self.file_counter
    }

    pub fn none_left(&self) -> bool {
        self.remaining() == 0
    }
}

impl IterateEntries for ZipFileInput {
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

struct AllInputZips {
    zips_todo: VecDeque<ZipFileInput>,
    cur_zip: UnsafeCell<ZipFileInput>,
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
                    .map(ZipFileInput::new)
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
    stdin_stream: Option<UnsafeCell<StdinInput>>,
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
                Some(UnsafeCell::new(StdinInput::new()))
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
