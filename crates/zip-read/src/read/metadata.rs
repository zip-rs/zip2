//! Central-directory metadata parsing used by ZIP readers.

use std::io::{self, Read};

use zip_core_lib::CompressionMethod;
use zip_core_lib::cp437::FromCp437;
use zip_core_lib::datetime::DateTime;
use zip_core_lib::extra_fields::ExtraFields;
use zip_core_lib::format::blocks::{
    CentralDirectoryEndInfo, DataAndPosition, ZipCentralEntryBlock,
};
use zip_core_lib::format::flags::{ZipFileFlags, ZipFlags};
use zip_core_lib::format::system::System;
use zip_core_lib::result::{ZipError, ZipResult, invalid};
use zip_core_lib::types::ZipFileData;

#[derive(Debug)]
pub struct CentralDirectoryInfo {
    pub archive_offset: u64,
    pub directory_start: u64,
    pub directory_size: u64,
    pub number_of_files: usize,
    pub disk_number: u32,
    pub disk_with_central_directory: u32,
}

impl<'a> TryFrom<&'a CentralDirectoryEndInfo> for CentralDirectoryInfo {
    type Error = ZipError;

    fn try_from(value: &'a CentralDirectoryEndInfo) -> Result<Self, Self::Error> {
        let (
            relative_cd_offset,
            directory_size,
            number_of_files,
            disk_number,
            disk_with_central_directory,
        ) = match &value.eocd64 {
            Some(DataAndPosition { data: eocd64, .. }) => {
                if eocd64.number_of_files_on_this_disk > eocd64.number_of_files {
                    return Err(invalid!(
                        "ZIP64 footer indicates more files on this disk than in the whole archive"
                    ));
                }
                (
                    eocd64.central_directory_offset,
                    eocd64.central_directory_size,
                    eocd64.number_of_files as usize,
                    eocd64.disk_number,
                    eocd64.disk_with_central_directory,
                )
            }
            _ => (
                u64::from(value.eocd.data.central_directory_offset),
                u64::from(value.eocd.data.central_directory_size),
                value.eocd.data.number_of_files_on_this_disk as usize,
                u32::from(value.eocd.data.disk_number),
                u32::from(value.eocd.data.disk_with_central_directory),
            ),
        };

        let directory_start = relative_cd_offset
            .checked_add(value.archive_offset)
            .ok_or(invalid!("Invalid central directory size or offset"))?;

        Ok(Self {
            archive_offset: value.archive_offset,
            directory_start,
            directory_size,
            number_of_files,
            disk_number,
            disk_with_central_directory,
        })
    }
}

#[inline]
fn read_variable_length_byte_field<R: Read>(reader: &mut R, len: usize) -> ZipResult<Vec<u8>> {
    if len == 0 {
        return Ok(Vec::new());
    }
    let mut data = vec![0; len];
    if let Err(e) = reader.read_exact(&mut data) {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            return Err(invalid!(
                "Variable-length field extends beyond file boundary"
            ));
        }
        return Err(e.into());
    }
    Ok(data)
}

/// Parse a central directory entry to collect the information for the file.
#[doc(hidden)]
pub fn central_header_to_zip_file_inner<R: Read>(
    reader: &mut R,
    archive_offset: u64,
    central_header_start: u64,
    block: ZipCentralEntryBlock,
) -> ZipResult<(ZipFileData, Vec<u8>)> {
    let ZipCentralEntryBlock {
        // magic,
        version_made_by,
        // version_to_extract,
        flags,
        compression_method,
        last_mod_time,
        last_mod_date,
        crc32,
        compressed_size,
        uncompressed_size,
        file_name_length,
        extra_field_length,
        file_comment_length,
        // disk_number,
        // internal_file_attributes,
        external_file_attributes,
        offset,
        ..
    } = block;

    let is_utf8 = ZipFlags::matching(flags, ZipFlags::LanguageEncoding);

    let mut file_name_raw = read_variable_length_byte_field(reader, file_name_length as usize)?;
    let extra_fields_raw = read_variable_length_byte_field(reader, extra_field_length as usize)?;
    let file_comment: Box<str> = if file_comment_length == 0 {
        Box::default()
    } else {
        let file_comment_raw =
            read_variable_length_byte_field(reader, file_comment_length as usize)?;
        if is_utf8 {
            String::from_utf8_lossy(&file_comment_raw).into()
        } else {
            file_comment_raw.from_cp437()?.into()
        }
    };

    let (version_made_by, system) = System::extract_bytes(version_made_by);
    let extra_fields = ExtraFields::parse(&extra_fields_raw, &block, false)?;
    // Construct the result
    let mut result = ZipFileData::from_central_directory(
        system,
        version_made_by,
        CompressionMethod::parse_from_u16(compression_method),
        DateTime::try_from_msdos(last_mod_date, last_mod_time).ok(),
        crc32,
        compressed_size.into(),
        uncompressed_size.into(),
        ZipFileFlags(flags),
        file_comment,
        offset.into(),
        central_header_start,
        external_file_attributes,
        extra_fields,
    );
    result.apply_extra_fields(&mut file_name_raw)?;

    // Account for shifted zip offsets.
    result.header_start = result
        .header_start
        .checked_add(archive_offset)
        .ok_or(invalid!("Archive header is too large"))?;

    Ok((result, file_name_raw))
}
