use core::mem::{offset_of, size_of};
use std::io::{Seek, SeekFrom, Write};
use zip_core_lib::ExtraField;
use zip_core_lib::datetime::DateTime;
use zip_core_lib::extra_fields::{DataStreamAlignment, Zip64ExtendedInformation};
use zip_core_lib::format::blocks::{FixedSizeBlock, ZipCentralEntryBlock, ZipLocalEntryBlock};
use zip_core_lib::format::magic::Magic;
use zip_core_lib::format::{ZIP64_BYTES_THR, ZIP64_BYTES_THR_U32};
use zip_core_lib::result::{ZipError, ZipResult, invalid};
use zip_core_lib::types::ZipFileData;

/// Write-specific header serialization methods for shared ZIP file metadata.
pub trait ZipFileWriteExt {
    /// Updates the local file header with the entry's finalized sizes and CRC.
    fn update_local_file_header<T: Write + Seek>(
        &mut self,
        writer: &mut T,
        file_name_raw: &[u8],
    ) -> ZipResult<()>;

    /// Writes the entry's central directory header.
    fn write_central_directory_header<T: Write>(
        &mut self,
        writer: &mut T,
        file_name_raw: &[u8],
    ) -> ZipResult<()>;

    /// Writes the entry's local header.
    fn write_local_header<T: Write>(
        &mut self,
        header_start: u64,
        writer: &mut T,
        file_name_raw: &[u8],
        alignment_opt: Option<(u16, u16)>,
    ) -> ZipResult<()>;
}

impl ZipFileWriteExt for ZipFileData {
    fn update_local_file_header<T: Write + Seek>(
        &mut self,
        writer: &mut T,
        file_name_raw: &[u8],
    ) -> ZipResult<()> {
        writer.seek(SeekFrom::Start(
            self.header_start + (size_of::<Magic>() + offset_of!(ZipLocalEntryBlock, crc32)) as u64,
        ))?;
        writer.write_all(&self.crc32.to_le_bytes())?;
        if self.large_file {
            debug_assert!(matches!(
                self.extra_fields.local_extra_fields().next(),
                Some(ExtraField::Zip64ExtendedInformation(_))
            ));
            writer.write_all(&ZIP64_BYTES_THR_U32.to_le_bytes())?;
            writer.write_all(&ZIP64_BYTES_THR_U32.to_le_bytes())?;
            update_local_zip64_extra_field(self, writer, file_name_raw)?;
        } else {
            // check compressed size as well as it can also be slightly larger than uncompressed size
            if self.compressed_size >= ZIP64_BYTES_THR {
                return Err(ZipError::Io(std::io::Error::other(
                    "large_file(true) option has not been set",
                )));
            }
            debug_assert!(
                !self
                    .extra_fields
                    .local_extra_fields()
                    .any(|field| matches!(field, ExtraField::Zip64ExtendedInformation(_)))
            );
            writer.write_all(&(self.compressed_size as u32).to_le_bytes())?;
            // uncompressed size is already checked on write to catch it as soon as possible
            writer.write_all(&(self.uncompressed_size as u32).to_le_bytes())?;
        }
        Ok(())
    }

    fn write_central_directory_header<T: Write>(
        &mut self,
        writer: &mut T,
        file_name_raw: &[u8],
    ) -> ZipResult<()> {
        // Rebuild the ZIP64 extra field from the entry's current sizes and offset so it carries
        // exactly the values the central header marks with the 0xFFFFFFFF sentinel (APPNOTE
        // 4.5.3), and nothing more. The previous code edited an existing field in place and set
        // both sizes unconditionally, so an entry whose offset alone needed ZIP64 got a field
        // with size values the header did not mark; a reader that follows the spec then takes the
        // first stored value (a size) as the offset. Recomputing (instead of editing in place)
        // also makes repeated writes of this header identical, which `finalize` relies on when it
        // writes the central directory and then rewrites it at the end of the file.
        self.extra_fields
            .inner
            .retain(|field| !matches!(field, ExtraField::Zip64ExtendedInformation(_)));
        if let Some(zip64_block) = Zip64ExtendedInformation::central_header(
            self.large_file,
            self.uncompressed_size,
            self.compressed_size,
            self.header_start,
        ) {
            self.extra_fields
                .inner
                .insert(0, ExtraField::Zip64ExtendedInformation(zip64_block));
        }
        // The ZIP64 field stores the two sizes together, so when it carries them both size
        // fields of the header must hold the sentinel, not only the one that overflowed;
        // otherwise the field has one more value than the header marks.
        let sizes_in_zip64 = self.extra_fields.inner.iter().any(
            |field| matches!(field, ExtraField::Zip64ExtendedInformation(z) if z.sizes.is_some()),
        );
        let central_extra_fields = self.extra_fields.central_extra_fields();
        let extra_field_len: usize = self
            .extra_fields
            .central_extra_fields()
            .map(|x| x.size(false))
            .sum();
        let compressed_size = if self.large_file || sizes_in_zip64 {
            ZIP64_BYTES_THR as u32
        } else {
            self.compressed_size
                .min(ZIP64_BYTES_THR)
                .try_into()
                .map_err(std::io::Error::other)?
        };
        let uncompressed_size = if self.large_file || sizes_in_zip64 {
            ZIP64_BYTES_THR as u32
        } else {
            self.uncompressed_size
                .min(ZIP64_BYTES_THR)
                .try_into()
                .map_err(std::io::Error::other)?
        };
        let offset = self
            .header_start
            .min(ZIP64_BYTES_THR)
            .try_into()
            .map_err(std::io::Error::other)?;
        let last_modified_time = self
            .last_modified_time
            .unwrap_or_else(DateTime::default_for_write);
        let version_to_extract = self.version_needed();
        let version_made_by = u16::from(self.version_made_by).max(version_to_extract);
        let compression_method = self.extern_compression();
        let extra_field_length = u16::try_from(extra_field_len)
            .map_err(|_| invalid!("Extra field length in central directory exceeds 64KiB"))?;
        let block = ZipCentralEntryBlock {
            version_made_by: ((self.system as u16) << 8) | version_made_by,
            version_to_extract,
            flags: self.flags(file_name_raw),
            compression_method,
            last_mod_time: last_modified_time.timepart(),
            last_mod_date: last_modified_time.datepart(),
            crc32: self.crc32,
            compressed_size,
            uncompressed_size,
            file_name_length: file_name_raw
                .len()
                .try_into()
                .map_err(std::io::Error::other)?,
            extra_field_length,
            file_comment_length: self
                .file_comment
                .len()
                .try_into()
                .map_err(std::io::Error::other)?,
            disk_number: 0,
            internal_file_attributes: 0,
            external_file_attributes: self.external_attributes,
            offset,
        };

        block.write(writer)?;
        // file name
        writer.write_all(file_name_raw)?;
        // extra field
        for one_extra_field in central_extra_fields {
            one_extra_field.write(writer, false)?;
        }
        // file comment
        writer.write_all(self.file_comment.as_bytes())?;
        Ok(())
    }

    fn write_local_header<T: Write>(
        &mut self,
        header_start: u64,
        writer: &mut T,
        file_name_raw: &[u8],
        alignment_opt: Option<(u16, u16)>,
    ) -> ZipResult<()> {
        self.extra_fields
            .inner
            .retain(|field| !matches!(field, ExtraField::Zip64ExtendedInformation { .. }));
        if let Some(zip64) = Zip64ExtendedInformation::local_header(
            self.large_file,
            self.uncompressed_size,
            self.compressed_size,
        ) {
            self.extra_fields
                .inner
                .insert(0, ExtraField::Zip64ExtendedInformation(zip64));
        }
        let (compressed_size, uncompressed_size) = if self.is_using_data_descriptor() {
            (0, 0)
        } else {
            (
                self.clamp_size_field(self.compressed_size)?,
                self.clamp_size_field(self.uncompressed_size)?,
            )
        };
        let mut extra_field_len: usize = self
            .extra_fields
            .local_extra_fields()
            .map(|x| x.size(true))
            .sum();
        let last_modified_time = self
            .last_modified_time
            .unwrap_or_else(DateTime::default_for_write);
        let compression_method = self.extern_compression();
        let header_end = header_start
            + (size_of::<Magic>() + size_of::<ZipLocalEntryBlock>()) as u64
            + file_name_raw.len() as u64;
        let opt_alignment_field = if let Some((alignment, header_id)) = alignment_opt
            && alignment > 1
        {
            let extra_field_end = header_end + extra_field_len as u64;
            let align = u64::from(alignment);
            let unaligned_header_bytes = extra_field_end % align;
            if unaligned_header_bytes != 0 {
                let mut pad_length = (align - unaligned_header_bytes) as usize;
                while pad_length < 6 {
                    pad_length += align as usize;
                }
                extra_field_len = self
                    .extra_fields
                    .local_extra_fields()
                    .map(|x| x.size(true))
                    .sum();
                let new_len = extra_field_len + pad_length;
                if new_len > u16::MAX as usize {
                    // Alignment is impossible without exceeding extra field size limits.
                    // Skip alignment.
                    None
                } else {
                    let pad_length_u16: Result<u16, core::num::TryFromIntError> =
                        pad_length.try_into();
                    if let Ok(pad_length) = pad_length_u16
                        && let Some(alignment_extra_field) = DataStreamAlignment::new_with_header_id(
                            header_id, pad_length, alignment,
                        )
                    {
                        // Add an extra field to the extra_field, per APPNOTE 4.6.11
                        extra_field_len = self
                            .extra_fields
                            .local_extra_fields()
                            .map(|x| x.size(true))
                            .sum::<usize>()
                            + alignment_extra_field.full_size(true);
                        debug_assert_eq!((extra_field_len as u64 + header_end) % align, 0);
                        Some(alignment_extra_field)
                    } else {
                        // Alignment is impossible with this length
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };
        let data_start = header_end + extra_field_len as u64;
        self.data_start.take();
        self.data_start.get_or_init(|| data_start);
        let block = ZipLocalEntryBlock {
            version_made_by: self.version_needed(),
            flags: self.flags(file_name_raw),
            compression_method,
            last_mod_time: last_modified_time.timepart(),
            last_mod_date: last_modified_time.datepart(),
            crc32: self.crc32,
            compressed_size,
            uncompressed_size,
            file_name_length: file_name_raw
                .len()
                .try_into()
                .map_err(std::io::Error::other)?,
            extra_field_length: extra_field_len
                .try_into()
                .map_err(|_| invalid!("Extra data field is too large"))?,
        };
        block.write(writer)?;
        writer.write_all(file_name_raw)?;
        let local_extra_fields = self.extra_fields.local_extra_fields_mut();
        #[cfg(feature = "aes-crypto")]
        let mut bytes_written = 0;
        for one_extra_field in local_extra_fields {
            one_extra_field.write(&mut *writer, true)?;
            #[cfg(feature = "aes-crypto")]
            {
                use zip_core_lib::extra_fields::AexEncryption;

                if let ExtraField::AeXEncryption(AexEncryption {
                    aes_extra_field_start,
                    ..
                }) = one_extra_field
                {
                    *aes_extra_field_start = Some(header_end as usize + bytes_written);
                }
                bytes_written += one_extra_field.size(true);
            }
        }

        if let Some(alignment_extra_field) = opt_alignment_field {
            alignment_extra_field.write(writer, true)?;
        }
        Ok(())
    }
}

fn update_local_zip64_extra_field<T: Write + Seek>(
    file: &mut ZipFileData,
    writer: &mut T,
    file_name_raw: &[u8],
) -> ZipResult<()> {
    let zip64_block = Zip64ExtendedInformation::local_header(
        file.large_file,
        file.uncompressed_size,
        file.compressed_size,
    )
    .ok_or(invalid!(
        "Attempted to update a nonexistent ZIP64 extra field"
    ))?;

    let zip64_extra_field_start = file.header_start
        + (size_of::<Magic>() + size_of::<ZipLocalEntryBlock>()) as u64
        + file_name_raw.len() as u64;

    writer.seek(SeekFrom::Start(zip64_extra_field_start))?;
    zip64_block.write(writer, true)?;
    Ok(())
}
