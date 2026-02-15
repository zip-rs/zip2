//! Iterable zip reader

use std::{
    borrow::Cow,
    io::{Read, Seek, SeekFrom},
};

use crate::{
    read::{
        central_header_to_zip_file, find_content, make_crypto_reader, make_reader,
        unsupported_zip_error, CentralDirectoryInfo, Config, ZipFile,
    },
    result::{ZipError, ZipResult},
    spec,
    types::ZipFileData,
    ZipReadOptions,
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
                    return Err(ZipError::UnsupportedArchive(ZipError::PASSWORD_REQUIRED))
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
}

impl<R: Read + Seek> Iterator for IterableZipFiles<R> {
    type Item = ZipResult<ZipFileData>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_file >= self.central_directory.number_of_files {
            return None;
        }
        self.current_file += 1;
        Some(central_header_to_zip_file(
            &mut self.reader,
            &self.central_directory,
        ))
    }
}
