//! Append, merge, and reader-dependent copy operations for ZIP archives.

use std::io::{Cursor, Read, Seek, SeekFrom, Write};

use zip_core_lib::format::flags::ZipFlags;
use zip_core_lib::result::{ZipError, ZipResult, invalid};
use zip_core_lib::types::ZipRawValues;
use zip_read::read::{Config, ZipArchive, ZipFile, ZipFileDataReadExt};
use zip_write::write::FullFileOptions;
pub use zip_write::write::ZipWriter;

/// Extension trait exposing append, merge, and archive-copy operations on [`ZipWriter`].
///
/// Import this trait to use these methods with the shared writer type.
pub trait ZipUpdate: Sized {
    /// The backing store for this writer.
    type Writer: Read + Write + Seek;

    /// Opens an existing ZIP archive for appending.
    fn new_append(readwriter: Self::Writer) -> ZipResult<Self>;

    /// Opens an existing ZIP archive for appending with custom read configuration.
    fn new_append_with_config(config: Config, readwriter: Self::Writer) -> ZipResult<Self>;

    /// Finishes writing and returns a readable archive over the same backing store.
    fn finish_into_readable(self) -> ZipResult<ZipArchive<Self::Writer>>;

    /// Merges the contents of another archive without decompressing or recompressing entries.
    fn merge_archive<R: Read + Seek>(&mut self, source: ZipArchive<R>) -> ZipResult<()>;

    /// Copies an entry already in this archive to a new name, copying its compressed bytes.
    fn deep_copy_file(&mut self, source_name: &str, destination_name: &str) -> ZipResult<()>;

    /// Adds a compressed entry from another archive with a new name.
    fn raw_copy_file_rename<R: Read, S: ToString>(
        &mut self,
        file: ZipFile<'_, R>,
        name: S,
    ) -> ZipResult<()>;

    /// Adds a compressed entry from another archive with its original name and encoding flag.
    fn raw_copy_file<R: Read>(&mut self, file: ZipFile<'_, R>) -> ZipResult<()>;

    /// Adds a compressed entry while overriding its timestamp and optional Unix permissions.
    fn raw_copy_file_touch<R: Read>(
        &mut self,
        file: ZipFile<'_, R>,
        last_modified_time: zip_core_lib::DateTime,
        unix_mode: Option<u32>,
    ) -> ZipResult<()>;
}

impl<W: Read + Write + Seek> ZipUpdate for ZipWriter<W> {
    type Writer = W;

    fn new_append(readwriter: W) -> ZipResult<Self> {
        Self::new_append_with_config(Config::default(), readwriter)
    }

    fn new_append_with_config(config: Config, mut readwriter: W) -> ZipResult<Self> {
        readwriter.seek(SeekFrom::Start(0))?;
        let metadata = ZipArchive::get_metadata(config, &mut readwriter)?;
        Ok(ZipWriter::from_append_metadata(
            readwriter,
            metadata.files,
            metadata.comment,
            metadata.zip64_extensible_data_sector,
        ))
    }

    fn finish_into_readable(self) -> ZipResult<ZipArchive<W>> {
        ZipArchive::new(self.finish()?)
    }

    fn merge_archive<R: Read + Seek>(&mut self, mut source: ZipArchive<R>) -> ZipResult<()> {
        let output_start = self.archive_append_position()?;
        let existing: std::collections::HashSet<Box<[u8]>> =
            self.existing_file_names().into_iter().collect();
        if let Some(name) = source
            .file_names_raw()
            .find(|name| existing.contains(*name))
        {
            return Err(invalid!("Duplicate filename: {:?}", name.escape_ascii()));
        }

        let mut files = source.shared.files.clone();
        files.values_mut().try_for_each(|file| {
            file.header_start = file.header_start.checked_add(output_start).ok_or(invalid!(
                "new header start from merge would have been too large"
            ))?;
            file.central_header_start = 0;
            if let Some(old_data_start) = file.data_start.take() {
                let new_data_start = old_data_start.checked_add(output_start).ok_or(invalid!(
                    "new data start from merge would have been too large"
                ))?;
                file.data_start.get_or_init(|| new_data_start);
            }
            Ok::<_, ZipError>(())
        })?;

        source.reader.rewind()?;
        self.append_archive_contents(&mut source.reader, source.shared.dir_start)?;
        self.append_archive_files(files);
        Ok(())
    }

    fn deep_copy_file(&mut self, source_name: &str, destination_name: &str) -> ZipResult<()> {
        let source_data_start =
            self.with_file_data_and_inner(source_name.as_bytes(), |reader, data| {
                let write_position = reader.stream_position()?;
                let data_start = data.data_start(reader)?;
                reader.seek(SeekFrom::Start(write_position))?;
                Ok(data_start)
            })?;
        self.deep_copy_file_from_data_start(source_name, destination_name, source_data_start)
    }

    fn raw_copy_file_rename<R: Read, S: ToString>(
        &mut self,
        mut file: ZipFile<'_, R>,
        name: S,
    ) -> ZipResult<()> {
        let options = raw_copy_options(&file)?;
        let name = name.to_string();
        let raw_values = ZipRawValues {
            crc32: file.crc32(),
            compressed_size: file.compressed_size(),
            uncompressed_size: file.size(),
            language_encoding: None,
        };
        self.add_raw_file(
            name.as_bytes(),
            options,
            raw_values,
            file.take_raw_reader()?,
        )
    }

    fn raw_copy_file<R: Read>(&mut self, mut file: ZipFile<'_, R>) -> ZipResult<()> {
        let options = raw_copy_options(&file)?;
        let name = file.name_raw().to_owned();
        let raw_values = ZipRawValues {
            crc32: file.crc32(),
            compressed_size: file.compressed_size(),
            uncompressed_size: file.size(),
            language_encoding: Some(file.flags().matching(ZipFlags::LanguageEncoding)),
        };
        self.add_raw_file(&name, options, raw_values, file.take_raw_reader()?)
    }

    fn raw_copy_file_touch<R: Read>(
        &mut self,
        mut file: ZipFile<'_, R>,
        last_modified_time: zip_core_lib::DateTime,
        unix_mode: Option<u32>,
    ) -> ZipResult<()> {
        let mut options = raw_copy_options(&file)?.last_modified_time(last_modified_time);
        if let Some(permissions) = unix_mode {
            let file_type = file.unix_mode().map_or(0, |mode| mode & !0o7777);
            options.external_attributes = None;
            options = options.unix_permissions(permissions);
            options.permissions = options.permissions.map(|mode| mode | file_type);
        }
        options.normalize();
        let name = file.name_raw().to_owned();
        let raw_values = ZipRawValues {
            crc32: file.crc32(),
            compressed_size: file.compressed_size(),
            uncompressed_size: file.size(),
            language_encoding: Some(file.flags().matching(ZipFlags::LanguageEncoding)),
        };
        self.add_raw_file(&name, options, raw_values, file.take_raw_reader()?)
    }
}

fn raw_copy_options<R: Read>(
    file: &ZipFile<'_, R>,
) -> ZipResult<FullFileOptions<'static, 'static>> {
    let mut options = file.options().into_full_options();
    if !file.comment().is_empty() {
        options = options.with_file_comment(file.comment());
    }
    for extra in file.extra_data_fields() {
        let mut buffer = Cursor::new(Vec::new());
        extra.write(&mut buffer, false)?;
        let buffer = buffer.into_inner();
        if buffer.len() >= 4 {
            let header_id = u16::from_le_bytes([buffer[0], buffer[1]]);
            #[cfg(feature = "aes-crypto")]
            if header_id == 0x9901 {
                continue;
            }
            options.add_extra_field(header_id, &buffer[4..], false)?;
        }
    }
    Ok(options)
}

/// Opens an existing ZIP archive for appending.
pub fn new_append<A: Read + Write + Seek>(readwriter: A) -> ZipResult<ZipWriter<A>> {
    <ZipWriter<A> as ZipUpdate>::new_append(readwriter)
}

/// Opens an existing ZIP archive for appending with custom read configuration.
pub fn new_append_with_config<A: Read + Write + Seek>(
    config: Config,
    readwriter: A,
) -> ZipResult<ZipWriter<A>> {
    <ZipWriter<A> as ZipUpdate>::new_append_with_config(config, readwriter)
}
