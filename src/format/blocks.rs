//! Blocks used in zip format

use crate::format::magic::Magic;
use crate::result::{ZipError, ZipResult, invalid};

use core::any::type_name;
use core::mem;
use core::slice;
use std::io::{self, Read, Write};

/// # Safety
///
/// - No padding/uninit bytes
/// - All bytes patterns must be valid
/// - No cell, pointers
///
/// See `bytemuck::Pod` for more details.
pub(crate) unsafe trait Pod: Copy + 'static {
    #[inline]
    fn zeroed() -> Self {
        unsafe { mem::zeroed() }
    }

    #[inline]
    fn as_bytes(&self) -> &[u8] {
        unsafe {
            slice::from_raw_parts(
                std::ptr::from_ref::<Self>(self).cast::<u8>(),
                mem::size_of::<Self>(),
            )
        }
    }

    #[inline]
    fn as_bytes_mut(&mut self) -> &mut [u8] {
        unsafe {
            slice::from_raw_parts_mut(
                std::ptr::from_mut::<Self>(self).cast::<u8>(),
                mem::size_of::<Self>(),
            )
        }
    }
}

#[derive(Copy, Clone)]
#[repr(C, packed)]
struct BlockWithMagic<T: FixedSizeBlock> {
    magic: Magic,
    inner: T,
}
unsafe impl<T: FixedSizeBlock> Pod for BlockWithMagic<T> {}

impl<T: FixedSizeBlock> BlockWithMagic<T> {
    fn to_le(mut self) -> Self {
        self.magic = self.magic.to_le();
        self.inner = self.inner.to_le();
        self
    }
}

pub(crate) trait FixedSizeBlock: Pod {
    const MAGIC: Magic;

    const WRONG_MAGIC_ERROR: ZipError;

    #[allow(clippy::wrong_self_convention)]
    fn from_le(self) -> Self;

    fn parse<R: Read + ?Sized>(reader: &mut R) -> ZipResult<Self> {
        let mut block_with_magic = BlockWithMagic::zeroed();
        if let Err(e) = reader.read_exact(block_with_magic.as_bytes_mut()) {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Err(invalid!("Unexpected end of {}", type_name::<Self>()));
            }
            return Err(e.into());
        }
        let BlockWithMagic {
            magic,
            inner: block,
        } = block_with_magic;
        let magic = Magic::from_le(magic);
        if magic != Self::MAGIC {
            return Err(Self::WRONG_MAGIC_ERROR);
        }
        let block = Self::from_le(block);
        Ok(block)
    }

    fn to_le(self) -> Self;

    fn write<T: Write + ?Sized>(self, writer: &mut T) -> ZipResult<()> {
        let block = BlockWithMagic {
            magic: Self::MAGIC,
            inner: self,
        };
        let block = block.to_le();
        writer.write_all(block.as_bytes())?;
        Ok(())
    }
}

/// Convert all the fields of a struct *from* little-endian representations.
macro_rules! from_le {
    ($obj:ident, $field:ident, $type:ty) => {
        $obj.$field = <$type>::from_le($obj.$field);
    };
    ($obj:ident, [($field:ident, $type:ty) $(,)?]) => {
        from_le![$obj, $field, $type];
    };
    ($obj:ident, [($field:ident, $type:ty), $($rest:tt),+ $(,)?]) => {
        from_le![$obj, $field, $type];
        from_le!($obj, [$($rest),+]);
    };
}

/// Convert all the fields of a struct *into* little-endian representations.
macro_rules! to_le {
    ($obj:ident, $field:ident, $type:ty) => {
        $obj.$field = <$type>::to_le($obj.$field);
    };
    ($obj:ident, [($field:ident, $type:ty) $(,)?]) => {
        to_le![$obj, $field, $type];
    };
    ($obj:ident, [($field:ident, $type:ty), $($rest:tt),+ $(,)?]) => {
        to_le![$obj, $field, $type];
        to_le!($obj, [$($rest),+]);
    };
}

/* TODO: derive macro to generate these fields? */
/// Implement `from_le()` and `to_le()`, providing the field specification to both macros
/// and methods.
macro_rules! to_and_from_le {
    ($($args:tt),+ $(,)?) => {
        #[inline(always)]
        fn from_le(mut self) -> Self {
            from_le![self, [$($args),+]];
            self
        }
        #[inline(always)]
        fn to_le(mut self) -> Self {
            to_le![self, [$($args),+]];
            self
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

unsafe impl Pod for ZipCentralEntryBlock {}

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

unsafe impl Pod for ZipLocalEntryBlock {}

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
pub struct ZipDataDescriptorBlock {
    pub crc32: u32,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
}

unsafe impl Pod for ZipDataDescriptorBlock {}

impl FixedSizeBlock for ZipDataDescriptorBlock {
    const MAGIC: Magic = Magic::DATA_DESCRIPTOR_SIGNATURE;

    const WRONG_MAGIC_ERROR: ZipError = invalid!("Invalid data descriptor header");

    to_and_from_le![
        (crc32, u32),
        (compressed_size, u32),
        (uncompressed_size, u32),
    ];
}

#[derive(Copy, Clone, Debug)]
#[repr(packed, C)]
#[allow(missing_docs)]
pub struct Zip64DataDescriptorBlock {
    pub crc32: u32,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
}

unsafe impl Pod for Zip64DataDescriptorBlock {}

impl FixedSizeBlock for Zip64DataDescriptorBlock {
    const MAGIC: Magic = Magic::DATA_DESCRIPTOR_SIGNATURE;

    const WRONG_MAGIC_ERROR: ZipError = invalid!("Invalid zip64 data descriptor header");

    to_and_from_le![
        (crc32, u32),
        (compressed_size, u64),
        (uncompressed_size, u64),
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

unsafe impl Pod for Zip32CDEBlock {}

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

unsafe impl Pod for Zip64CDELocatorBlock {}

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

unsafe impl Pod for Zip64CDEBlock {}

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

    pub(crate) fn parse<T: Read + ?Sized>(
        reader: &mut T,
        max_size: u64,
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
        } else if record_size.saturating_add(Self::RECORD_OVERHEAD) > max_size {
            return Err(invalid!("EOCD64 extends beyond EOCD64 locator"));
        }

        let zip64_extensible_data_sector = if record_size > (Self::MIN_SIZE as u64) {
            let mut extensible_data_sector =
                vec![0u8; record_size as usize - Self::MIN_SIZE].into_boxed_slice();
            if let Err(e) = reader.read_exact(&mut extensible_data_sector) {
                if e.kind() == io::ErrorKind::UnexpectedEof {
                    return Err(invalid!(
                        "EOCD64 extensible data sector exceeds file boundary"
                    ));
                }
                return Err(e.into());
            }
            Some(extensible_data_sector)
        } else {
            None
        };

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
            zip64_extensible_data_sector,
        })
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use crate::{
        format::blocks::{FixedSizeBlock, Pod},
        format::magic::Magic,
        result::{ZipError, invalid},
    };

    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
    #[repr(packed, C)]
    pub struct TestBlock {
        pub file_name_length: u16,
    }

    unsafe impl Pod for TestBlock {}

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
}
