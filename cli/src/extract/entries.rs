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

pub fn make_entry_iterator<'a>(
    input_type: InputType,
) -> Result<Box<dyn IterateEntries + 'a>, CommandError> {
    let ret: Box<dyn IterateEntries + 'a> = match input_type {
        InputType::StreamingStdin => Box::new(StdinInput::new()),
        InputType::ZipPaths(zip_paths) => Box::new(AllInputZips::new(zip_paths)?),
    };
    Ok(ret)
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
