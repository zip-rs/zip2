//! Iterable zip reader

use crate::format::blocks::{FixedSizeBlock, ZipCentralEntryBlock};
use crate::format::find_central_directory_end;
use crate::read::{ZipFileEntry, central_header_to_zip_file_inner};
use crate::{
    read::{CentralDirectoryInfo, Config},
    result::{ZipError, ZipResult},
};
use std::{
    borrow::Cow,
    io::{Read, Seek, SeekFrom},
};

/// Iterable version of ZipArchive
pub struct ZipIterable<R> {
    #[allow(unused)]
    pub(crate) config: Config,
    pub(crate) iterable_files: ZipIterableFiles<R>,
}
impl<R: Read + Seek> ZipIterable<R> {
    /// Try to create a new zip archive
    pub fn try_new(mut reader: R, config: Config) -> ZipResult<ZipIterable<R>> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        let mut end_exclusive = file_len;
        let mut last_err = None;

        let central_directory = loop {
            let cde = match find_central_directory_end(
                &mut reader,
                config.archive_offset,
                end_exclusive,
                file_len,
            ) {
                Ok(cde) => cde,
                Err(e) => return Err(last_err.unwrap_or(e)),
            };

            match CentralDirectoryInfo::try_from(&cde) {
                Ok(info) => break info,
                Err(e) => {
                    last_err = Some(e);
                    end_exclusive = cde.eocd.position;
                }
            }
        };

        // If the parsed number of files is greater than the offset then
        // something fishy is going on and we shouldn't trust number_of_files.
        if central_directory.number_of_files > central_directory.directory_start as usize {
            return Err(ZipError::UnsupportedArchive("Fishy error :)"));
        }

        if central_directory.disk_number != central_directory.disk_with_central_directory {
            return Err(ZipError::UnsupportedArchive(
                "Support for multi-disk files is not implemented",
            ));
        }

        let iterable_shared = ZipIterableFiles::try_new(reader, central_directory)?;

        Ok(Self {
            config,
            iterable_files: iterable_shared,
        })
    }

    /// Get the file as an iterator
    pub fn files(&mut self) -> ZipResult<&mut ZipIterableFiles<R>> {
        self.iterable_files.reset()?;
        Ok(&mut self.iterable_files)
    }
}

/// Iterable Files
#[derive(Debug)]
pub struct ZipIterableFiles<R> {
    reader: R,
    central_directory: CentralDirectoryInfo,
    current_file: usize,
}

impl<R: Read + Seek> ZipIterableFiles<R> {
    /// Try to create an iterable of files
    pub(crate) fn try_new(
        mut reader: R,
        central_directory: CentralDirectoryInfo,
    ) -> ZipResult<Self> {
        reader.seek(SeekFrom::Start(central_directory.directory_start))?;
        Ok(Self {
            reader,
            central_directory,
            current_file: 0,
        })
    }

    pub(crate) fn reset(&mut self) -> ZipResult<()> {
        self.current_file = 0;
        self.reader
            .seek(SeekFrom::Start(self.central_directory.directory_start))?;
        Ok(())
    }

    pub(crate) fn parse_entry(&mut self) -> ZipResult<ZipFileEntry<'static>> {
        let central_header_start = self.reader.stream_position()?;

        // Parse central header
        let block = ZipCentralEntryBlock::parse(&mut self.reader)?;
        let (data, file_name) = central_header_to_zip_file_inner(
            &mut self.reader,
            self.central_directory.archive_offset,
            central_header_start,
            block,
        )?;
        let file = ZipFileEntry {
            file_name_raw: Cow::Owned(file_name),
            data: Cow::Owned(data),
        };
        let central_header_end = self.reader.stream_position()?;

        self.reader.seek(SeekFrom::Start(central_header_end))?;
        Ok(file)
    }
}

impl<R: Read + Seek> Iterator for ZipIterableFiles<R> {
    type Item = ZipResult<ZipFileEntry<'static>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_file >= self.central_directory.number_of_files {
            return None;
        }
        self.current_file += 1;
        let file = self.parse_entry();
        Some(file)
    }
}
