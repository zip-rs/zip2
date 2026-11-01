use std::borrow::Cow;
use std::io::{Read, Seek, SeekFrom, Take};
use std::mem::size_of;

use zip_core_lib::extra_fields::ExtraFields;
use zip_core_lib::format::ZIP64_BYTES_THR;
use zip_core_lib::format::blocks::{FixedSizeBlock, ZipLocalEntryBlock};
use zip_core_lib::format::data_descriptor::{
    Zip64DataDescriptorBlock, ZipDataDescriptor, ZipDataDescriptorBlock,
};
use zip_core_lib::format::magic::Magic;
use zip_core_lib::read::ZipReadOptions;
use zip_core_lib::result::{ZipError, ZipResult, invalid};
use zip_core_lib::types::ZipFileData;

use super::readers::{SeekableTake, ZipFileReader, make_crypto_reader, make_reader};

pub trait ZipFileDataReadExt {
    fn make_reader<'a, R: Read>(
        &self,
        limit_reader: Take<&'a mut R>,
        options: ZipReadOptions<'_>,
    ) -> ZipResult<ZipFileReader<'a, R>>;
    fn data_start(&self, reader: &mut (impl Read + Seek + ?Sized)) -> ZipResult<u64>;
    fn find_content<'a, R: Read + Seek + ?Sized>(
        &self,
        reader: &'a mut R,
    ) -> ZipResult<Take<&'a mut R>>;
    fn find_content_seek<'a, R: Read + Seek + ?Sized>(
        &self,
        reader: &'a mut R,
    ) -> ZipResult<SeekableTake<'a, R>>;
    fn validate_header_consistency<R: Read + Seek + ?Sized>(
        &self,
        reader: &mut R,
        expected_name: &[u8],
    ) -> ZipResult<()>;
    fn get_data_descriptor<R: Read + Seek>(
        &self,
        reader: &mut R,
    ) -> ZipResult<Option<ZipDataDescriptor>>;
}

impl ZipFileDataReadExt for ZipFileData {
    fn make_reader<'a, R: Read>(
        &self,
        limit_reader: Take<&'a mut R>,
        mut options: ZipReadOptions<'_>,
    ) -> ZipResult<ZipFileReader<'a, R>> {
        if options.ignore_encryption_flag {
            // Always use no password when we're ignoring the encryption flag.
            options.password = None;
        } else {
            // Require and use the password only if the file is encrypted.
            match (options.password, self.is_encrypted()) {
                (None, true) => {
                    return Err(ZipError::UnsupportedArchive(ZipError::PASSWORD_REQUIRED));
                }
                // Password supplied, but none needed! Discard.
                (Some(_), false) => options.password = None,
                _ => {}
            }
        }
        let compression_method = self.compression_method;
        let uncompressed_size = self.uncompressed_size;
        let crc32 = if options.ignore_crc {
            None
        } else {
            Some(self.crc32)
        };
        // The declared size bounds the output, unless it is not known (a streamed entry with
        // a data descriptor, see `read_zipfile_from_stream`).
        let size_limit = (!options.size_unknown).then_some(uncompressed_size);
        let crypto_reader = make_crypto_reader(self, limit_reader, options.password)?;
        let aes_vendor_version = self.aes_settings().map(|aes| aes.1);
        make_reader(
            compression_method,
            uncompressed_size,
            size_limit,
            crc32,
            aes_vendor_version,
            crypto_reader,
            #[cfg(feature = "legacy-zip")]
            self.flags.as_u16(),
        )
    }

    fn data_start(&self, reader: &mut (impl Read + Seek + ?Sized)) -> ZipResult<u64> {
        match self.data_start.get() {
            Some(data_start) => Ok(*data_start),
            None => {
                // Go to start of data.
                reader.seek(SeekFrom::Start(self.header_start))?;

                // Parse static-sized fields and check the magic value.
                let block = ZipLocalEntryBlock::parse(reader)?;

                // Each of these fields must be converted to u64 before adding, as the result may
                // easily overflow a u16.
                let variable_fields_len =
                    u64::from(block.file_name_length) + u64::from(block.extra_field_length);
                // Calculate the end of the local header from the fields we just parsed.
                let data_start = self
                    .header_start
                    .checked_add((size_of::<Magic>() + size_of::<ZipLocalEntryBlock>()) as u64)
                    .and_then(|v| v.checked_add(variable_fields_len))
                    .ok_or(ZipError::InvalidArchive(Cow::Borrowed(
                        "Local header end offset overflows u64",
                    )))?;

                // Set the value so we don't have to read it again.
                match self.data_start.set(data_start) {
                    Ok(()) => (),
                    // If the value was already set in the meantime, ensure it matches (this is probably
                    // unnecessary).
                    Err(existing_value) => {
                        debug_assert_eq!(existing_value, data_start);
                    }
                }

                Ok(data_start)
            }
        }
    }

    fn find_content<'a, R: Read + Seek + ?Sized>(
        &self,
        reader: &'a mut R,
    ) -> ZipResult<Take<&'a mut R>> {
        // TODO: use .get_or_try_init() once stabilized to provide a closure returning a Result!
        let data_start = self.data_start(reader)?;
        check_data_end(self, data_start)?;
        reader.seek(SeekFrom::Start(data_start))?;

        Ok(reader.take(self.compressed_size))
    }

    fn find_content_seek<'a, R: Read + Seek + ?Sized>(
        &self,
        reader: &'a mut R,
    ) -> ZipResult<SeekableTake<'a, R>> {
        // Parse local header
        let data_start = self.data_start(reader)?;
        check_data_end(self, data_start)?;
        reader.seek(SeekFrom::Start(data_start))?;

        // Explicit Ok and ? are needed to convert io::Error to ZipError
        Ok(SeekableTake::new(reader, self.compressed_size)?)
    }

    fn validate_header_consistency<R: Read + Seek + ?Sized>(
        &self,
        reader: &mut R,
        expected_name: &[u8],
    ) -> ZipResult<()> {
        let current_pos = reader.stream_position()?;
        reader.seek(SeekFrom::Start(self.header_start))?;

        let block = match ZipLocalEntryBlock::parse(reader) {
            Ok(b) => b,
            Err(e) => {
                let _ = reader.seek(SeekFrom::Start(current_pos));
                return Err(e);
            }
        };

        let mut local_name = vec![0u8; block.file_name_length as usize];
        if let Err(e) = reader.read_exact(&mut local_name) {
            let _ = reader.seek(SeekFrom::Start(current_pos));
            return Err(e.into());
        }

        let mut local_extra = vec![0u8; block.extra_field_length as usize];
        if let Err(e) = reader.read_exact(&mut local_extra) {
            let _ = reader.seek(SeekFrom::Start(current_pos));
            return Err(e.into());
        }

        // Restore reader position
        reader.seek(SeekFrom::Start(current_pos))?;

        let variable_fields_len =
            u64::from(block.file_name_length) + u64::from(block.extra_field_length);
        let data_start = self
            .header_start
            .checked_add((size_of::<Magic>() + size_of::<ZipLocalEntryBlock>()) as u64)
            .and_then(|v| v.checked_add(variable_fields_len))
            .ok_or(ZipError::InvalidArchive(Cow::Borrowed(
                "Local header end offset overflows u64",
            )))?;
        let _ = self.data_start.set(data_start);

        let local_extra_fields = ExtraFields::parse(&local_extra, &block, true)?;
        let mut local_data = Self::from_local_block(block, local_extra_fields)?;
        local_data.apply_extra_fields(&mut local_name)?;

        let local_meta = local_data.consistency_metadata(&local_name, true);
        let central_meta = self.consistency_metadata(expected_name, false);

        local_meta.validate_matches(&central_meta)
    }

    fn get_data_descriptor<R: Read + Seek>(
        &self,
        reader: &mut R,
    ) -> ZipResult<Option<ZipDataDescriptor>> {
        if !self.is_using_data_descriptor() {
            return Ok(None);
        }
        let current = reader.stream_position()?;
        let data_start = match self.data_start(reader) {
            Ok(data_start) => data_start,
            Err(_) => return Ok(None),
        };
        let is_zip64 = self.compressed_size >= ZIP64_BYTES_THR
            || self.uncompressed_size >= ZIP64_BYTES_THR
            || self.extra_fields.inner.iter().any(|field| {
                matches!(field, zip_core_lib::ExtraField::Zip64ExtendedInformation(_))
            });
        let result =
            (|| {
                let desc_start = data_start.checked_add(self.compressed_size).ok_or(
                    ZipError::InvalidArchive(Cow::Borrowed("Data descriptor offset overflows u64")),
                )?;
                reader.seek(SeekFrom::Start(desc_start))?;
                if is_zip64 {
                    let mut buffer = [0; Zip64DataDescriptorBlock::SIZE];
                    reader.read_exact(&mut buffer)?;
                    Zip64DataDescriptorBlock::parse(&buffer)
                        .map(ZipDataDescriptor::Zip64DataDescriptorBlock)
                        .map_err(|message| ZipError::InvalidArchive(Cow::Borrowed(message)))
                } else {
                    let mut buffer = [0; ZipDataDescriptorBlock::SIZE];
                    reader.read_exact(&mut buffer)?;
                    ZipDataDescriptorBlock::parse(&buffer)
                        .map(ZipDataDescriptor::ZipDataDescriptorBlock)
                        .map_err(|message| ZipError::InvalidArchive(Cow::Borrowed(message)))
                }
            })();
        reader.seek(SeekFrom::Start(current))?;
        result.map(Some).map_err(Into::into)
    }
}

fn check_data_end(data: &ZipFileData, data_start: u64) -> ZipResult<u64> {
    data_start
        .checked_add(data.compressed_size)
        .ok_or(invalid!("File data end offset overflows u64"))
}
