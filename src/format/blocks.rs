//! Blocks used in zip format

use crate::format::magic::Magic;
use crate::result::{ZipError, ZipResult, invalid};

use core::any::type_name;
use std::io::{self, Read, Write};

/// A fixed-size block: a magic, followed by little-endian fields without padding.
///
/// Use [`to_and_from_le!`] to implement the encoding.
pub(crate) trait FixedSizeBlock: Copy + 'static {
    const MAGIC: Magic;

    const WRONG_MAGIC_ERROR: ZipError;

    /// The magic and the fields, as they're stored in the archive.
    type Bytes: AsRef<[u8]> + AsMut<[u8]>;

    const ZEROED_BYTES: Self::Bytes;

    /// Decodes the magic, which may be wrong, and the fields.
    fn from_bytes(bytes: &Self::Bytes) -> (Magic, Self);

    /// Encodes [`Self::MAGIC`] and the fields.
    fn to_bytes(self) -> Self::Bytes;

    #[inline]
    fn zeroed() -> Self {
        let (_, block) = Self::from_bytes(&Self::ZEROED_BYTES);
        block
    }

    fn parse<R: Read + ?Sized>(reader: &mut R) -> ZipResult<Self> {
        let mut bytes = Self::ZEROED_BYTES;
        if let Err(e) = reader.read_exact(bytes.as_mut()) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Err(invalid!("Unexpected end of {}", type_name::<Self>()));
            }
            return Err(e.into());
        }
        let (magic, block) = Self::from_bytes(&bytes);
        if magic != Self::MAGIC {
            return Err(Self::WRONG_MAGIC_ERROR);
        }
        Ok(block)
    }

    /// Parses a block whose magic has already been read and checked.
    fn parse_after_magic<R: Read + ?Sized>(reader: &mut R) -> io::Result<Self> {
        let mut bytes = Self::ZEROED_BYTES;
        let (_, fields) = bytes.as_mut().split_at_mut(size_of::<Magic>());
        reader.read_exact(fields)?;
        let (_, block) = Self::from_bytes(&bytes);
        Ok(block)
    }

    fn write<T: Write + ?Sized>(self, writer: &mut T) -> ZipResult<()> {
        writer.write_all(self.to_bytes().as_ref())?;
        Ok(())
    }
}

/// Returns the `N` bytes at `OFFSET`, which are checked to be in bounds at compile time.
#[inline(always)]
fn get_bytes<const OFFSET: usize, const N: usize, const LEN: usize>(bytes: &[u8; LEN]) -> [u8; N] {
    const { assert!(OFFSET + N <= LEN) };
    let mut field = [0; N];
    field.copy_from_slice(&bytes[OFFSET..OFFSET + N]);
    field
}

/// Replaces the `N` bytes at `OFFSET`, which are checked to be in bounds at compile time.
#[inline(always)]
fn set_bytes<const OFFSET: usize, const N: usize, const LEN: usize>(
    bytes: &mut [u8; LEN],
    field: [u8; N],
) {
    const { assert!(OFFSET + N <= LEN) };
    bytes[OFFSET..OFFSET + N].copy_from_slice(&field);
}

/* TODO: derive macro to generate these fields? */
/// Implements the encoding of a [`FixedSizeBlock`], given all the fields of a `repr(packed, C)` struct.
macro_rules! to_and_from_le {
    ($(($field:ident, $type:ty)),+ $(,)?) => {
        type Bytes = [u8; size_of::<Magic>() + size_of::<Self>()];

        const ZEROED_BYTES: Self::Bytes = [0; size_of::<Magic>() + size_of::<Self>()];

        #[inline(always)]
        fn from_bytes(bytes: &Self::Bytes) -> (Magic, Self) {
            use $crate::format::blocks::get_bytes;
            use core::mem::offset_of;

            // There's no padding.
            const { assert!(size_of::<Self>() == 0 $(+ size_of::<$type>())+) };

            let magic = Magic::from_le_bytes(get_bytes::<0, _, _>(bytes));
            let block = Self {
                $(
                    $field: <$type>::from_le_bytes(get_bytes::<
                        { size_of::<Magic>() + offset_of!(Self, $field) }, _, _
                    >(bytes)),
                )+
            };
            (magic, block)
        }

        #[inline(always)]
        fn to_bytes(self) -> Self::Bytes {
            use $crate::format::blocks::set_bytes;
            use core::mem::offset_of;

            let mut bytes = Self::ZEROED_BYTES;
            set_bytes::<0, _, _>(&mut bytes, Self::MAGIC.to_le_bytes());
            $(
                set_bytes::<{ size_of::<Magic>() + offset_of!(Self, $field) }, _, _>(
                    &mut bytes,
                    <$type>::to_le_bytes(self.$field),
                );
            )+
            bytes
        }
    };
}

#[derive(Copy, Clone, Debug)]
#[repr(packed, C)]
#[allow(missing_docs)]
pub struct ZipCentralEntryBlock {
    pub version_made_by: u16,
    pub version_to_extract: u16,
    pub flags: u16,
    pub compression_method: u16,
    pub last_mod_time: u16,
    pub last_mod_date: u16,
    pub crc32: u32,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub file_name_length: u16,
    pub extra_field_length: u16,
    pub file_comment_length: u16,
    pub disk_number: u16,
    pub internal_file_attributes: u16,
    pub external_file_attributes: u32,
    pub offset: u32,
}

impl ZipCentralEntryBlock {
    /// Parses the 46-byte fixed central-directory header, including its signature.
    ///
    /// Checks the signature and converts little-endian fields. Does not consume
    /// the variable-length name, extra fields, or comment. Callers must validate
    /// lengths, offsets, and archive policy separately; this does not validate or
    /// authenticate a complete archive. This is part of the unstable API.
    pub fn parse<R: Read + ?Sized>(reader: &mut R) -> ZipResult<Self> {
        <Self as FixedSizeBlock>::parse(reader)
    }
}

impl ZipEntryBlock for ZipCentralEntryBlock {
    fn get_uncompressed_size(&self) -> u32 {
        self.uncompressed_size
    }
    fn get_compressed_size(&self) -> u32 {
        self.compressed_size
    }
    fn get_header_start(&self) -> Option<u32> {
        Some(self.offset)
    }
}

impl FixedSizeBlock for ZipCentralEntryBlock {
    const MAGIC: Magic = Magic::CENTRAL_DIRECTORY_HEADER_SIGNATURE;

    const WRONG_MAGIC_ERROR: ZipError = invalid!("Invalid Central Directory header");

    to_and_from_le![
        (version_made_by, u16),
        (version_to_extract, u16),
        (flags, u16),
        (compression_method, u16),
        (last_mod_time, u16),
        (last_mod_date, u16),
        (crc32, u32),
        (compressed_size, u32),
        (uncompressed_size, u32),
        (file_name_length, u16),
        (extra_field_length, u16),
        (file_comment_length, u16),
        (disk_number, u16),
        (internal_file_attributes, u16),
        (external_file_attributes, u32),
        (offset, u32),
    ];
}

pub(crate) trait ZipEntryBlock {
    fn get_uncompressed_size(&self) -> u32;
    fn get_compressed_size(&self) -> u32;
    fn get_header_start(&self) -> Option<u32>;
}

#[derive(Copy, Clone, Debug)]
#[repr(packed, C)]
#[allow(missing_docs)]
pub struct ZipLocalEntryBlock {
    pub version_made_by: u16,
    pub flags: u16,
    pub compression_method: u16,
    pub last_mod_time: u16,
    pub last_mod_date: u16,
    pub crc32: u32,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub file_name_length: u16,
    pub extra_field_length: u16,
}

impl ZipLocalEntryBlock {
    /// Parses the 30-byte fixed local-file header, including its signature.
    ///
    /// Checks the signature and converts little-endian fields. Does not consume
    /// the variable-length name or extra fields. Callers must validate lengths,
    /// offsets, and archive policy separately; this does not validate or
    /// authenticate a complete archive. This is part of the unstable API.
    pub fn parse<R: Read + ?Sized>(reader: &mut R) -> ZipResult<Self> {
        <Self as FixedSizeBlock>::parse(reader)
    }
}

impl ZipEntryBlock for ZipLocalEntryBlock {
    fn get_uncompressed_size(&self) -> u32 {
        self.uncompressed_size
    }
    fn get_compressed_size(&self) -> u32 {
        self.compressed_size
    }
    fn get_header_start(&self) -> Option<u32> {
        None
    }
}

impl FixedSizeBlock for ZipLocalEntryBlock {
    const MAGIC: Magic = Magic::LOCAL_FILE_HEADER_SIGNATURE;

    const WRONG_MAGIC_ERROR: ZipError = invalid!("Invalid local file header");

    to_and_from_le![
        (version_made_by, u16),
        (flags, u16),
        (compression_method, u16),
        (last_mod_time, u16),
        (last_mod_date, u16),
        (crc32, u32),
        (compressed_size, u32),
        (uncompressed_size, u32),
        (file_name_length, u16),
        (extra_field_length, u16),
    ];
}

#[derive(Copy, Clone, Debug)]
#[repr(packed, C)]
#[allow(missing_docs)]
pub struct Zip32CDEBlock {
    pub disk_number: u16,
    pub disk_with_central_directory: u16,
    pub number_of_files_on_this_disk: u16,
    pub number_of_files: u16,
    pub central_directory_size: u32,
    pub central_directory_offset: u32,
    pub zip_file_comment_length: u16,
}

impl FixedSizeBlock for Zip32CDEBlock {
    const MAGIC: Magic = Magic::CENTRAL_DIRECTORY_END_SIGNATURE;

    const WRONG_MAGIC_ERROR: ZipError = invalid!("Invalid Central Directory End header");

    to_and_from_le![
        (disk_number, u16),
        (disk_with_central_directory, u16),
        (number_of_files_on_this_disk, u16),
        (number_of_files, u16),
        (central_directory_size, u32),
        (central_directory_offset, u32),
        (zip_file_comment_length, u16)
    ];
}

#[derive(Debug)]
#[allow(missing_docs)]
pub struct Zip32CentralDirectoryEnd {
    pub disk_number: u16,
    pub disk_with_central_directory: u16,
    pub number_of_files_on_this_disk: u16,
    pub number_of_files: u16,
    pub central_directory_size: u32,
    pub central_directory_offset: u32,
    pub zip_file_comment: Box<[u8]>,
}

impl Zip32CentralDirectoryEnd {
    fn into_block_and_comment(self) -> (Zip32CDEBlock, Box<[u8]>) {
        let Self {
            disk_number,
            disk_with_central_directory,
            number_of_files_on_this_disk,
            number_of_files,
            central_directory_size,
            central_directory_offset,
            zip_file_comment,
        } = self;
        let block = Zip32CDEBlock {
            disk_number,
            disk_with_central_directory,
            number_of_files_on_this_disk,
            number_of_files,
            central_directory_size,
            central_directory_offset,
            zip_file_comment_length: zip_file_comment.len() as u16,
        };

        (block, zip_file_comment)
    }

    /// Parse the block
    pub fn parse<T: Read + ?Sized>(reader: &mut T) -> ZipResult<Zip32CentralDirectoryEnd> {
        let Zip32CDEBlock {
            disk_number,
            disk_with_central_directory,
            number_of_files_on_this_disk,
            number_of_files,
            central_directory_size,
            central_directory_offset,
            zip_file_comment_length,
            ..
        } = Zip32CDEBlock::parse(reader)?;

        let mut zip_file_comment = vec![0u8; zip_file_comment_length as usize].into_boxed_slice();
        if let Err(e) = reader.read_exact(&mut zip_file_comment) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Err(invalid!("EOCD comment exceeds file boundary"));
            }

            return Err(e.into());
        }

        Ok(Zip32CentralDirectoryEnd {
            disk_number,
            disk_with_central_directory,
            number_of_files_on_this_disk,
            number_of_files,
            central_directory_size,
            central_directory_offset,
            zip_file_comment,
        })
    }

    /// Write the block
    pub fn write<T: Write>(self, writer: &mut T) -> ZipResult<()> {
        let (block, comment) = self.into_block_and_comment();

        if comment.len() > u16::MAX as usize {
            return Err(invalid!("EOCD comment length exceeds u16::MAX"));
        }

        block.write(writer)?;
        writer.write_all(&comment)?;
        Ok(())
    }

    /// Check if the zip file could be a zip64
    #[must_use]
    pub fn may_be_zip64(&self) -> bool {
        self.number_of_files == u16::MAX
            || self.central_directory_size == u32::MAX
            || self.central_directory_offset == u32::MAX
    }
}

#[derive(Copy, Clone)]
#[repr(packed, C)]
#[allow(missing_docs)]
pub struct Zip64CDELocatorBlock {
    pub disk_with_central_directory: u32,
    pub end_of_central_directory_offset: u64,
    pub number_of_disks: u32,
}

impl FixedSizeBlock for Zip64CDELocatorBlock {
    const MAGIC: Magic = Magic::ZIP64_CENTRAL_DIRECTORY_END_LOCATOR_SIGNATURE;

    const WRONG_MAGIC_ERROR: ZipError =
        invalid!("Invalid ZIP64 End of Central Directory Locator header");

    to_and_from_le![
        (disk_with_central_directory, u32),
        (end_of_central_directory_offset, u64),
        (number_of_disks, u32),
    ];
}

#[allow(missing_docs)]
pub struct Zip64CentralDirectoryEndLocator {
    pub disk_with_central_directory: u32,
    pub end_of_central_directory_offset: u64,
    pub number_of_disks: u32,
}

impl Zip64CentralDirectoryEndLocator {
    /// Parse
    pub fn parse<T: Read + ?Sized>(reader: &mut T) -> ZipResult<Zip64CentralDirectoryEndLocator> {
        let Zip64CDELocatorBlock {
            disk_with_central_directory,
            end_of_central_directory_offset,
            number_of_disks,
            ..
        } = Zip64CDELocatorBlock::parse(reader)?;

        Ok(Zip64CentralDirectoryEndLocator {
            disk_with_central_directory,
            end_of_central_directory_offset,
            number_of_disks,
        })
    }

    /// Get the block
    #[must_use]
    pub fn block(self) -> Zip64CDELocatorBlock {
        let Self {
            disk_with_central_directory,
            end_of_central_directory_offset,
            number_of_disks,
        } = self;
        Zip64CDELocatorBlock {
            disk_with_central_directory,
            end_of_central_directory_offset,
            number_of_disks,
        }
    }

    /// Write the block
    pub fn write<T: Write>(self, writer: &mut T) -> ZipResult<()> {
        self.block().write(writer)
    }
}

#[derive(Copy, Clone)]
#[repr(packed, C)]
#[allow(missing_docs)]
pub struct Zip64CDEBlock {
    pub record_size: u64,
    pub version_made_by: u16,
    pub version_needed_to_extract: u16,
    pub disk_number: u32,
    pub disk_with_central_directory: u32,
    pub number_of_files_on_this_disk: u64,
    pub number_of_files: u64,
    pub central_directory_size: u64,
    pub central_directory_offset: u64,
}

impl FixedSizeBlock for Zip64CDEBlock {
    const MAGIC: Magic = Magic::ZIP64_CENTRAL_DIRECTORY_END_SIGNATURE;

    const WRONG_MAGIC_ERROR: ZipError = invalid!("Invalid ZIP64 Central Directory End header");

    to_and_from_le![
        (record_size, u64),
        (version_made_by, u16),
        (version_needed_to_extract, u16),
        (disk_number, u32),
        (disk_with_central_directory, u32),
        (number_of_files_on_this_disk, u64),
        (number_of_files, u64),
        (central_directory_size, u64),
        (central_directory_offset, u64),
    ];
}

#[allow(missing_docs)]
pub struct Zip64CentralDirectoryEnd {
    pub record_size: u64,
    pub version_made_by: u16,
    pub version_needed_to_extract: u16,
    pub disk_number: u32,
    pub disk_with_central_directory: u32,
    pub number_of_files_on_this_disk: u64,
    pub number_of_files: u64,
    pub central_directory_size: u64,
    pub central_directory_offset: u64,
    pub(crate) zip64_extensible_data_sector: Option<Box<[u8]>>,
}

impl Zip64CentralDirectoryEnd {
    /// Minimum size of the block
    /// Block - record_size - extensible_data
    const MIN_SIZE: usize = 2 * size_of::<u16>() + 2 * size_of::<u32>() + 4 * size_of::<u64>();
    pub(crate) const MIN_FULL_SIZE: usize =
        2 * size_of::<u16>() + 2 * size_of::<u32>() + 5 * size_of::<u64>();
    /// Size of ZIP64 EOCD signature + record_size field.
    pub(crate) const RECORD_OVERHEAD: u64 = (size_of::<Magic>() + size_of::<u64>()) as u64;

    /// Parses the fixed part of the record, which must end exactly `expected_length` bytes after
    /// its signature. The extensible data sector is not read; call
    /// [`Self::read_extensible_data_sector`] once the record has been accepted, so that rejected
    /// candidates cost O(1) instead of O(`record_size`).
    pub(crate) fn parse_header<T: Read + ?Sized>(
        reader: &mut T,
        expected_length: u64,
    ) -> ZipResult<Zip64CentralDirectoryEnd> {
        let Zip64CDEBlock {
            record_size,
            version_made_by,
            version_needed_to_extract,
            disk_number,
            disk_with_central_directory,
            number_of_files_on_this_disk,
            number_of_files,
            central_directory_size,
            central_directory_offset,
            ..
        } = Zip64CDEBlock::parse(reader)?;

        if record_size < (Self::MIN_SIZE - size_of::<Magic>()) as u64 {
            return Err(invalid!("Low EOCD64 record size"));
        } else if record_size.checked_add(Self::RECORD_OVERHEAD) != Some(expected_length) {
            return Err(invalid!("Invalid EOCD64: inconsistent length"));
        }

        Ok(Self {
            record_size,
            version_made_by,
            version_needed_to_extract,
            disk_number,
            disk_with_central_directory,
            number_of_files_on_this_disk,
            number_of_files,
            central_directory_size,
            central_directory_offset,
            zip64_extensible_data_sector: None,
        })
    }

    /// Reads the extensible data sector that follows the fixed part parsed by
    /// [`Self::parse_header`]; `reader` must still be positioned right after it.
    pub(crate) fn read_extensible_data_sector<T: Read + ?Sized>(
        &mut self,
        reader: &mut T,
    ) -> ZipResult<()> {
        if self.record_size > (Self::MIN_SIZE as u64) {
            let mut extensible_data_sector =
                vec![0u8; self.record_size as usize - Self::MIN_SIZE].into_boxed_slice();
            if let Err(e) = reader.read_exact(&mut extensible_data_sector) {
                if e.kind() == io::ErrorKind::UnexpectedEof {
                    return Err(invalid!(
                        "EOCD64 extensible data sector exceeds file boundary"
                    ));
                }
                return Err(e.into());
            }
            self.zip64_extensible_data_sector = Some(extensible_data_sector);
        }
        Ok(())
    }

    pub(crate) fn into_block_and_extensible_data(self) -> (Zip64CDEBlock, Option<Box<[u8]>>) {
        let Self {
            record_size,
            version_made_by,
            version_needed_to_extract,
            disk_number,
            disk_with_central_directory,
            number_of_files_on_this_disk,
            number_of_files,
            central_directory_size,
            central_directory_offset,
            zip64_extensible_data_sector,
        } = self;

        (
            Zip64CDEBlock {
                record_size,
                version_made_by,
                version_needed_to_extract,
                disk_number,
                disk_with_central_directory,
                number_of_files_on_this_disk,
                number_of_files,
                central_directory_size,
                central_directory_offset,
            },
            zip64_extensible_data_sector,
        )
    }

    pub(crate) fn write<T: Write>(self, writer: &mut T) -> ZipResult<()> {
        let (block, zip64_extensible_data) = self.into_block_and_extensible_data();
        block.write(writer)?;
        if let Some(extensible_data) = zip64_extensible_data {
            writer.write_all(&extensible_data)?;
        }
        Ok(())
    }
}

pub(crate) struct DataAndPosition<T> {
    pub data: T,
    #[allow(dead_code)]
    pub position: u64,
}

impl<T> From<(T, u64)> for DataAndPosition<T> {
    fn from(value: (T, u64)) -> Self {
        Self {
            data: value.0,
            position: value.1,
        }
    }
}

pub(crate) struct CentralDirectoryEndInfo {
    pub eocd: DataAndPosition<Zip32CentralDirectoryEnd>,
    pub eocd64: Option<DataAndPosition<Zip64CentralDirectoryEnd>>,

    pub archive_offset: u64,
}

impl CentralDirectoryEndInfo {
    /// Whether the central directory ends exactly where this record (or its ZIP64 record) begins.
    pub(crate) fn central_directory_abuts_end(&self) -> bool {
        let (relative_offset, size, end) = match &self.eocd64 {
            Some(eocd64) => (
                eocd64.data.central_directory_offset,
                eocd64.data.central_directory_size,
                eocd64.position,
            ),
            None => (
                u64::from(self.eocd.data.central_directory_offset),
                u64::from(self.eocd.data.central_directory_size),
                self.eocd.position,
            ),
        };
        relative_offset
            .checked_add(self.archive_offset)
            .and_then(|start| start.checked_add(size))
            == Some(end)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, ErrorKind};

    use crate::{
        format::blocks::{
            FixedSizeBlock, Zip32CDEBlock, Zip64CDEBlock, Zip64CDELocatorBlock,
            ZipCentralEntryBlock, ZipLocalEntryBlock,
        },
        format::magic::Magic,
        result::{ZipError, invalid},
    };

    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
    #[repr(packed, C)]
    pub struct TestBlock {
        pub file_name_length: u16,
    }

    impl FixedSizeBlock for TestBlock {
        const MAGIC: Magic = Magic::literal(0x01111);

        const WRONG_MAGIC_ERROR: ZipError = invalid!("unreachable");

        to_and_from_le![(file_name_length, u16)];
    }

    /// Demonstrate that a block object can be safely written to memory and deserialized back out.
    #[test]
    fn block_serde() {
        let block = TestBlock {
            file_name_length: 3,
        };
        let mut c = Cursor::new(Vec::new());
        block.write(&mut c).unwrap();
        c.set_position(0);
        let block2 = TestBlock::parse(&mut c).unwrap();
        assert_eq!(block, block2);
    }

    #[test]
    fn test_size_zip64_central_directory_end() {
        use super::Zip64CentralDirectoryEnd;
        assert_eq!(Zip64CentralDirectoryEnd::MIN_SIZE, 44);
        assert_eq!(Zip64CentralDirectoryEnd::MIN_FULL_SIZE, 52);
    }

    /// `signature`, followed by `len` distinct bytes.
    ///
    /// The blocks below hold the same bytes in their fields, so a field with the wrong offset,
    /// width or byte order changes the result.
    fn encoded(signature: [u8; 4], len: u8) -> Vec<u8> {
        signature.into_iter().chain(1..=len).collect()
    }

    fn write_to_vec<T: FixedSizeBlock>(block: T) -> Vec<u8> {
        let mut bytes = Vec::new();
        block.write(&mut bytes).unwrap();
        bytes
    }

    fn check_encoding<T: FixedSizeBlock>(block: T, expected: &[u8]) {
        assert_eq!(expected.len(), size_of::<Magic>() + size_of::<T>());
        assert_eq!(write_to_vec(block), expected);

        // Parsing is the inverse of writing, and leaves what follows the block unread.
        let mut reader = Cursor::new([expected, b"rest"].concat());
        let parsed = T::parse(&mut reader).unwrap();
        assert_eq!(reader.position(), expected.len() as u64);
        assert_eq!(write_to_vec(parsed), expected);

        // A truncated block is rejected wherever the cut is.
        for len in 0..expected.len() {
            let error = T::parse(&mut &expected[..len]).err().unwrap();
            assert!(
                matches!(&error, ZipError::InvalidArchive(m) if m.starts_with("Unexpected end of ")),
                "{len} bytes: {error:?}"
            );
        }

        // So is a block with a different signature.
        for index in 0..size_of::<Magic>() {
            let mut bytes = expected.to_vec();
            bytes[index] ^= 0xff;
            let error = T::parse(&mut bytes.as_slice()).err().unwrap();
            assert_eq!(error.to_string(), T::WRONG_MAGIC_ERROR.to_string());
        }
    }

    #[test]
    fn central_entry_block_encoding() {
        check_encoding(
            ZipCentralEntryBlock {
                version_made_by: 0x0201,
                version_to_extract: 0x0403,
                flags: 0x0605,
                compression_method: 0x0807,
                last_mod_time: 0x0a09,
                last_mod_date: 0x0c0b,
                crc32: 0x100f_0e0d,
                compressed_size: 0x1413_1211,
                uncompressed_size: 0x1817_1615,
                file_name_length: 0x1a19,
                extra_field_length: 0x1c1b,
                file_comment_length: 0x1e1d,
                disk_number: 0x201f,
                internal_file_attributes: 0x2221,
                external_file_attributes: 0x2625_2423,
                offset: 0x2a29_2827,
            },
            &encoded(*b"PK\x01\x02", 42),
        );
    }

    #[test]
    fn local_entry_block_encoding() {
        check_encoding(
            ZipLocalEntryBlock {
                version_made_by: 0x0201,
                flags: 0x0403,
                compression_method: 0x0605,
                last_mod_time: 0x0807,
                last_mod_date: 0x0a09,
                crc32: 0x0e0d_0c0b,
                compressed_size: 0x1211_100f,
                uncompressed_size: 0x1615_1413,
                file_name_length: 0x1817,
                extra_field_length: 0x1a19,
            },
            &encoded(*b"PK\x03\x04", 26),
        );
    }

    #[test]
    fn zip32_cde_block_encoding() {
        check_encoding(
            Zip32CDEBlock {
                disk_number: 0x0201,
                disk_with_central_directory: 0x0403,
                number_of_files_on_this_disk: 0x0605,
                number_of_files: 0x0807,
                central_directory_size: 0x0c0b_0a09,
                central_directory_offset: 0x100f_0e0d,
                zip_file_comment_length: 0x1211,
            },
            &encoded(*b"PK\x05\x06", 18),
        );
    }

    #[test]
    fn zip64_cde_locator_block_encoding() {
        check_encoding(
            Zip64CDELocatorBlock {
                disk_with_central_directory: 0x0403_0201,
                end_of_central_directory_offset: 0x0c0b_0a09_0807_0605,
                number_of_disks: 0x100f_0e0d,
            },
            &encoded(*b"PK\x06\x07", 16),
        );
    }

    #[test]
    fn zip64_cde_block_encoding() {
        check_encoding(
            Zip64CDEBlock {
                record_size: 0x0807_0605_0403_0201,
                version_made_by: 0x0a09,
                version_needed_to_extract: 0x0c0b,
                disk_number: 0x100f_0e0d,
                disk_with_central_directory: 0x1413_1211,
                number_of_files_on_this_disk: 0x1c1b_1a19_1817_1615,
                number_of_files: 0x2423_2221_201f_1e1d,
                central_directory_size: 0x2c2b_2a29_2827_2625,
                central_directory_offset: 0x3433_3231_302f_2e2d,
            },
            &encoded(*b"PK\x06\x06", 52),
        );
    }

    /// Reading the magic separately gives the same block, and the same error as `read_exact` when
    /// the block is cut short.
    #[test]
    fn parse_after_magic() {
        let bytes = encoded(*b"PK\x03\x04", 26);
        let fields = &bytes[size_of::<Magic>()..];

        let mut reader = Cursor::new([fields, b"rest"].concat());
        let block = ZipLocalEntryBlock::parse_after_magic(&mut reader).unwrap();
        assert_eq!(reader.position(), fields.len() as u64);
        assert_eq!(write_to_vec(block), bytes);

        for len in 0..fields.len() {
            let error = ZipLocalEntryBlock::parse_after_magic(&mut &fields[..len]).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::UnexpectedEof);
        }
    }
}
