use indexmap::IndexMap;
use std::{
    io::Cursor,
    sync::{Arc, OnceLock},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWrite, SeekFrom};

use crate::{
    extra_fields::ExtendedTimestamp,
    result::{ZipError, ZipResult},
    spec,
    cp437::FromCp437,
    types::{AesVendorVersion, System, ZipFileData},
    AesMode, CompressionMethod, DateTime, ExtraField, ZipArchive,
};

use super::CentralDirectoryInfo;

impl<R> ZipArchive<R>
where
    R: AsyncRead + AsyncSeek + Unpin,
{
    pub(crate) async fn merge_contents<W>(
        &mut self,
        mut w: W,
    ) -> ZipResult<IndexMap<Box<str>, ZipFileData>>
    where
        W: AsyncWrite + AsyncSeek + Unpin,
    {
        if self.shared.files.is_empty() {
            return Ok(IndexMap::new());
        }
        let mut new_files = self.shared.files.clone();

        let new_initial_header_start = w.stream_position().await?;
        new_files.values_mut().try_for_each(|f| {
            f.header_start = f.header_start.checked_add(new_initial_header_start).ok_or(
                ZipError::InvalidArchive("new header start from merge would have been to large"),
            )?;
            f.central_header_start = 0;

            if let Some(old_data_start) = f.data_start.take() {
                let new_data_start = old_data_start.checked_add(new_initial_header_start).ok_or(
                    ZipError::InvalidArchive("new data start from merge would have been to large"),
                )?;
                f.data_start.get_or_init(|| new_data_start);
            }
            Ok::<_, ZipError>(())
        })?;

        self.reader.rewind().await?;

        let length_to_read = self.shared.dir_start;

        let mut limited_raw = (&mut self.reader as &mut R).take(length_to_read);
        tokio::io::copy(&mut limited_raw, &mut w).await?;

        Ok(new_files)
    }

    async fn get_directory_info_zip32(
        footer: &spec::CentralDirectoryEnd,
        cde_start_pos: u64,
    ) -> ZipResult<CentralDirectoryInfo> {
        let archive_offset = cde_start_pos
            .checked_sub(footer.central_directory_size as u64)
            .and_then(|x| x.checked_sub(footer.central_directory_offset as u64))
            .ok_or(ZipError::InvalidArchive(
                "Invalid central directory size or offset",
            ))?;
        let directory_start = footer.central_directory_offset as u64 + archive_offset;
        let number_of_files = footer.number_of_files_on_this_disk as usize;

        Ok(CentralDirectoryInfo {
            archive_offset,
            directory_start,
            number_of_files,
            disk_number: footer.disk_number as u32,
            disk_with_central_directory: footer.disk_with_central_directory as u32,
        })
    }

    pub async fn get_directory_info_zip64(
        reader: &mut R,
        footer: &spec::CentralDirectoryEnd,
        cde_start_pos: u64,
    ) -> ZipResult<Vec<ZipResult<CentralDirectoryInfo>>> {
        reader
            .seek(tokio::io::SeekFrom::End(
                -(20 + 22 + footer.zip_file_comment.len() as i64),
            ))
            .await?;

        let locator64 = spec::Zip64CentralDirectoryEndLocator::parse(reader).await?;

        let search_upper_bound = cde_start_pos
            .checked_sub(60)
            .ok_or(ZipError::InvalidArchive(
                "File cannot contain ZIP64 central directory end",
            ))?;

        let search_results = spec::Zip64CentralDirectoryEnd::find_and_parse(
            reader,
            locator64.end_of_central_directory_offset,
            search_upper_bound,
        )
        .await?;

        let results: Vec<Result<CentralDirectoryInfo, ZipError>> = search_results.iter().map(|(footer64, archive_offset)| {
            let directory_start_result = footer64.central_directory_offset.checked_add(*archive_offset).ok_or(ZipError::InvalidArchive("Invalid central directory size or effect"));
            directory_start_result.and_then(| directory_start| {
                if directory_start > search_upper_bound {
                    Err(ZipError::InvalidArchive("Invalid central directory size or offset"))
                } else if footer64.number_of_files_on_this_disk > footer64.number_of_files {
                    Err(ZipError::InvalidArchive("ZIP64 footer indicates more files on this disk then in the whole archive"))
                } else if footer64.version_needed_to_extract > footer64.version_made_by {
                    Err(ZipError::InvalidArchive("ZIP64 footer indicates a new version is needed to extract this archive than the \
                    version that wrote it"))
                } else {
                    Ok(CentralDirectoryInfo {
                        archive_offset: *archive_offset,
                        directory_start,
                        number_of_files: footer64.number_of_files as usize,
                        disk_number: footer64.disk_number,
                        disk_with_central_directory: footer64.disk_with_central_directory
                    })
                }
            })
        }).collect();
        Ok(results)
    }
}

pub(crate) async fn central_header_to_zip_file<R>(
    reader: &mut R,
    archive_offset: u64,
) -> ZipResult<ZipFileData>
where
    R: AsyncRead + AsyncSeek + Unpin,
{
    let central_header_start = reader.stream_position().await?;

    let signature = reader.read_u32_le().await?;
    if signature != spec::CENTRAL_DIRECTORY_HEADER_SIGNATURE {
        Err(ZipError::InvalidArchive("INvalid Central Directory header"))
    } else {
        central_header_to_zip_file_inner(reader, archive_offset, central_header_start).await
    }
}

async fn central_header_to_zip_file_inner<R>(
    reader: &mut R,
    archive_offset: u64,
    central_header_start: u64,
) -> ZipResult<ZipFileData>
where
    R: AsyncRead + Unpin,
{
    let version_made_by = reader.read_u16_le().await?;
    let _version_to_extract = reader.read_u16_le().await?;
    let flags = reader.read_u16_le().await?;
    let encrypted = flags & 1 == 1;
    let is_utf8 = flags & (1 << 11) != 0;
    let using_data_descriptor = flags & (1 << 3) != 0;
    let compression_method = reader.read_u16_le().await?;
    let last_mod_time = reader.read_u16_le().await?;
    let last_mod_date = reader.read_u16_le().await?;
    let crc32 = reader.read_u32_le().await?;
    let compressed_size = reader.read_u32_le().await?;
    let uncompressed_size = reader.read_u32_le().await?;
    let file_name_length = reader.read_u16_le().await? as usize;
    let extra_field_length = reader.read_u16_le().await? as usize;
    let file_comment_length = reader.read_u16_le().await? as usize;
    let _disk_number = reader.read_u16_le().await?;
    let _internal_file_attributes = reader.read_u16_le().await?;
    let external_file_attributes = reader.read_u32_le().await?;
    let offset = reader.read_u32_le().await? as u64;
    let mut file_name_raw = Vec::with_capacity(file_name_length);
    let mut extra_field = Vec::with_capacity(extra_field_length);
    let mut file_comment_raw = Vec::with_capacity(file_comment_length);
    reader.read_exact(&mut file_name_raw).await?;
    reader.read_exact(&mut extra_field).await?;
    reader.read_exact(&mut file_comment_raw).await?;

    let file_name: Box<str> = if is_utf8 {
        String::from_utf8_lossy(&file_name_raw).into()
    } else {
        file_name_raw.from_cp437().into()
    };
    let file_comment: Box<str> = if is_utf8 {
        String::from_utf8_lossy(&file_comment_raw).into()
    } else {
        file_comment_raw.from_cp437().into()
    };

    let mut result = ZipFileData {
        system: System::from((version_made_by >> 8) as u8),
        version_made_by: version_made_by as u8,
        encrypted,
        using_data_descriptor,
        compression_method: {
            #[allow(deprecated)]
            CompressionMethod::from_u16(compression_method)
        },
        compression_level: None,
        last_modified_time: DateTime::from_msdos(last_mod_date, last_mod_time),
        crc32,
        compressed_size: compressed_size as u64,
        uncompressed_size: uncompressed_size as u64,
        file_name,
        file_name_raw: file_name_raw.into(),
        extra_field: Some(Arc::new(extra_field)),
        central_extra_field: None,
        file_comment,
        header_start: offset,
        extra_data_start: None,
        central_header_start,
        data_start: OnceLock::new(),
        external_attributes: external_file_attributes,
        large_file: false,
        aes_mode: None,
        aes_extra_data_start: 0,
        extra_fields: Vec::new(),
    };

    match parse_extra_field(&mut result).await {
        Ok(..) | Err(ZipError::Io(..)) => {}
        Err(err) => return Err(err),
    }

    let aes_enabled = result.compression_method == CompressionMethod::AES;
    if aes_enabled && result.aes_mode.is_none() {
        return Err(ZipError::InvalidArchive(
            "AES encryption without AES extra data field",
        ));
    }

    result.header_start = result
        .header_start
        .checked_add(archive_offset)
        .ok_or(ZipError::InvalidArchive("Archive header is too large"))?;
    Ok(result)
}

pub async fn parse_extra_field(file: &mut ZipFileData) -> ZipResult<()> {
    let Some(extra_field) = &file.extra_field else {
        return Ok(());
    };
    let mut reader = Cursor::new(extra_field.as_ref());

    while (reader.position() as usize) < extra_field.len() {
        let kind = reader.read_u16_le().await?;
        let len = reader.read_u16_le().await?;
        let mut len_left = len as i64;
        match kind {
            0x001 => {
                if file.uncompressed_size == spec::ZIP64_BYTES_THR {
                    file.large_file = true;
                    file.uncompressed_size = reader.read_u64_le().await?;
                    len_left -= 8;
                }
                if file.compressed_size == spec::ZIP64_BYTES_THR {
                    file.large_file = true;
                    file.compressed_size = reader.read_u64_le().await?;
                    len_left -= 8;
                }
                if file.header_start == spec::ZIP64_BYTES_THR {
                    file.header_start = reader.read_u64_le().await?;
                    len_left -= 8;
                }
            }
            0x9901 => {
                if len != 7 {
                    return Err(ZipError::UnsupportedArchive(
                        "AES extra data field has an unsupported length",
                    ));
                    let vendor_version = reader.read_u16_le().await?;
                    let vendor_id = reader.read_u16_le().await?;
                    let mut out = [0u8];
                    reader.read_exact(&mut out).await?;
                    let aes_mode = out[0];
                    #[allow(deprecated)]
                    let compression_method =
                        CompressionMethod::from_u16(reader.read_u16_le().await?);

                    if vendor_id != 0x4541 {
                        return Err(ZipError::InvalidArchive("Invalid AES vendor"));
                    }
                    let vendor_version = match vendor_version {
                        0x0001 => AesVendorVersion::Ae1,
                        0x0002 => AesVendorVersion::Ae2,
                        _ => return Err(ZipError::InvalidArchive("Invalid AES vendor version")),
                    };
                    match aes_mode {
                        0x01 => {
                            file.aes_mode =
                                Some((AesMode::Aes128, vendor_version, compression_method))
                        }
                        0x02 => {
                            file.aes_mode =
                                Some((AesMode::Aes192, vendor_version, compression_method))
                        }
                        0x03 => {
                            file.aes_mode =
                                Some((AesMode::Aes256, vendor_version, compression_method))
                        }
                        _ => {
                            return Err(ZipError::InvalidArchive("Invalid AES encryption strength"))
                        }
                    };
                    file.compression_method = compression_method;
                }
            }
            0x5455 => {
                file.extra_fields.push(ExtraField::ExtendedTimestamp(
                    ExtendedTimestamp::try_from_reader(&mut reader, len)?,
                ));

                len_left = 0;
            }
            _ => {}
        }
        if len_left > 0 {
            reader.seek(SeekFrom::Current(len_left)).await?;
        }
    }
    Ok(())
}
