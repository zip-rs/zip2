//! Iterable zip reader

use crate::cp437::FromCp437;
use crate::read::central_header_to_zip_file_inner;
use crate::spec::ZipFlags;
use crate::{
    ZipReadOptions,
    read::{
        CentralDirectoryInfo, Config, ZipFile, find_content, make_crypto_reader, make_reader,
        read_variable_length_byte_field, unsupported_zip_error,
    },
    result::{ZipError, ZipResult},
    spec::{self, FixedSizeBlock},
    types::{ZipCentralEntryBlock, ZipFileData},
};
use std::{
    borrow::Cow,
    io::{Read, Seek, SeekFrom},
};

/// Iterable version of ZipArchive
pub struct IterableZip<R> {
    #[allow(unused)]
    pub(crate) config: Config,
    pub(crate) iterable_files: IterableZipFiles<R>,
}
impl<R: Read + Seek> IterableZip<R> {
    /// Try to create a new zip archive
    pub fn try_new(reader: R, config: Config) -> ZipResult<IterableZip<R>> {
        Self::with_config(config, reader)
    }

    fn with_config(config: Config, mut reader: R) -> ZipResult<IterableZip<R>> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        let mut end_exclusive = file_len;
        let mut last_err = None;

        let central_directory = loop {
            let cde = match spec::find_central_directory(
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
            return unsupported_zip_error("Fishy error :)");
        }

        if central_directory.disk_number != central_directory.disk_with_central_directory {
            return unsupported_zip_error("Support for multi-disk files is not implemented");
        }

        let iterable_shared = IterableZipFiles::try_new(reader, central_directory)?;

        Ok(IterableZip {
            config,
            iterable_files: iterable_shared,
        })
    }

    /// Get the file as an iterator
    pub fn files(&mut self) -> ZipResult<&mut IterableZipFiles<R>> {
        self.iterable_files.reset()?;
        Ok(&mut self.iterable_files)
    }

    /// Get a contained file by index with options.
    pub fn by_file_data<'data>(
        &'data mut self,
        data: &'data ZipFileData,
        mut options: ZipReadOptions<'_>,
    ) -> ZipResult<ZipFile<'data, R>> {
        if options.ignore_encryption_flag {
            // Always use no password when we're ignoring the encryption flag.
            options.password = None;
        } else {
            // Require and use the password only if the file is encrypted.
            match (options.password, data.encrypted) {
                (None, true) => {
                    return Err(ZipError::UnsupportedArchive(ZipError::PASSWORD_REQUIRED));
                }
                // Password supplied, but none needed! Discard.
                (Some(_), false) => options.password = None,
                _ => {}
            }
        }
        let limit_reader = find_content(data, &mut self.iterable_files.reader)?;

        let crypto_reader =
            make_crypto_reader(data, limit_reader, options.password, data.aes_mode)?;

        Ok(ZipFile {
            data: Cow::Borrowed(data),
            reader: make_reader(
                data.compression_method,
                data.uncompressed_size,
                data.crc32,
                crypto_reader,
                #[cfg(feature = "legacy-zip")]
                data.flags,
            )?,
        })
    }
}

/// Iterable Files
#[derive(Debug)]
pub struct IterableZipFiles<R> {
    reader: R,
    central_directory: CentralDirectoryInfo,
    current_file: usize,
}

impl<R: Read + Seek> IterableZipFiles<R> {
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

    pub(crate) fn parse_entry(&mut self) -> ZipResult<ZipEntry> {
        let central_header_start = self.reader.stream_position()?;

        // Parse central header
        let block = ZipCentralEntryBlock::parse(&mut self.reader)?;
        let variable_data =
            ZipCentralEntryVariableDataRaw::try_from_reader(&mut self.reader, &block)?;
        let file = ZipEntry::new(
            self.central_directory.archive_offset,
            block,
            variable_data,
            central_header_start,
        );
        let central_header_end = self.reader.stream_position()?;

        self.reader.seek(SeekFrom::Start(central_header_end))?;
        Ok(file)
    }
}

impl<R: Read + Seek> Iterator for IterableZipFiles<R> {
    type Item = ZipResult<ZipEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_file >= self.central_directory.number_of_files {
            return None;
        }
        self.current_file += 1;
        let file = self.parse_entry();
        Some(file)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ZipCentralEntryVariableDataRaw {
    file_name: Box<[u8]>,
    extra_fields: Box<[u8]>,
    file_comment: Box<[u8]>,
}

impl ZipCentralEntryVariableDataRaw {
    fn try_from_reader<R: Read>(reader: &mut R, block: &ZipCentralEntryBlock) -> ZipResult<Self> {
        let file_name_raw =
            read_variable_length_byte_field(reader, block.file_name_length as usize)?;
        let extra_field =
            read_variable_length_byte_field(reader, block.extra_field_length as usize)?;
        let file_comment_raw =
            read_variable_length_byte_field(reader, block.file_comment_length as usize)?;
        Ok(Self {
            file_name: file_name_raw,
            extra_fields: extra_field,
            file_comment: file_comment_raw,
        })
    }
}

/// A Zip entry
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ZipEntry {
    archive_offset: u64,
    central_block: ZipCentralEntryBlock,
    variable_data: ZipCentralEntryVariableDataRaw,
    central_block_start: u64,
}

impl ZipEntry {
    pub(crate) fn new(
        archive_offset: u64,
        central_block: ZipCentralEntryBlock,
        variable_data: ZipCentralEntryVariableDataRaw,
        start_offset: u64,
    ) -> Self {
        Self {
            archive_offset,
            central_block,
            variable_data,
            central_block_start: start_offset,
        }
    }
    /// Check if the entry have the UTF-8 encoding flag
    pub fn is_utf8(&self) -> bool {
        // TODO
        self.central_block.flags & (ZipFlags::LanguageEncoding as u16) != 0
    }

    /// Get file name
    pub fn file_name(&self) -> ZipResult<std::borrow::Cow<'_, str>> {
        let file_name_raw = self.file_name_raw();
        // TODO
        let file_name = if self.is_utf8() {
            String::from_utf8_lossy(file_name_raw)
        } else {
            file_name_raw.from_cp437()?
        };
        Ok(file_name)
    }

    /// Get raw file name
    pub fn file_name_raw(&self) -> &[u8] {
        &self.variable_data.file_name
    }

    /// TODO convert into zip_file
    pub fn into_zip_file_data<R: Read + std::io::Seek>(
        self,
        reader: &mut R,
    ) -> ZipResult<ZipFileData> {
        central_header_to_zip_file_inner(
            reader,
            self.archive_offset,
            self.central_block_start,
            self.central_block,
        )
    }
}
