//! Zip data descriptor

use crate::format::magic::Magic;
use core::mem;
use std::io::{self, Write};

/// Zip Data descriptor
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ZipDataDescriptor {
    /// Classic zip data descriptor
    ZipDataDescriptorBlock(ZipDataDescriptorBlock),
    /// Zip64 data descriptor
    Zip64DataDescriptorBlock(Zip64DataDescriptorBlock),
}

/// Zip data descriptor
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ZipDataDescriptorBlock {
    /// crc 32
    pub crc32: u32,
    /// compressed size
    pub compressed_size: u32,
    /// uncompressed size
    pub uncompressed_size: u32,
}

impl ZipDataDescriptorBlock {
    /// Signature + fields
    pub const SIZE: usize = mem::size_of::<u32>()
        + mem::size_of::<u32>()
        + mem::size_of::<u32>()
        + mem::size_of::<u32>();
    /// Magic
    pub const MAGIC: Magic = Magic::DATA_DESCRIPTOR_SIGNATURE;

    /// Parse the zip64 data descriptor
    pub fn parse(buff: &[u8; Self::SIZE]) -> Result<Self, &'static str> {
        let magic = Magic::from_le_bytes([buff[0], buff[1], buff[2], buff[3]]);
        if magic != Self::MAGIC {
            return Err("Invalid data descriptor header");
        }
        let crc32 = u32::from_le_bytes([buff[4], buff[5], buff[6], buff[7]]);
        let compressed_size = u32::from_le_bytes([buff[8], buff[9], buff[10], buff[11]]);
        let uncompressed_size = u32::from_le_bytes([buff[12], buff[13], buff[14], buff[15]]);

        Ok(Self {
            crc32,
            compressed_size,
            uncompressed_size,
        })
    }

    /// Write the zip64 data descriptor
    pub fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(&Self::MAGIC.to_le_bytes())?;
        w.write_all(&self.crc32.to_le_bytes())?;
        w.write_all(&self.compressed_size.to_le_bytes())?;
        w.write_all(&self.uncompressed_size.to_le_bytes())?;
        Ok(())
    }
}

/// Zip64 data descriptor
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Zip64DataDescriptorBlock {
    /// crc 32
    pub crc32: u32,
    /// compressed size
    pub compressed_size: u64,
    /// uncompressed size
    pub uncompressed_size: u64,
}
impl Zip64DataDescriptorBlock {
    /// Signature + fields
    pub const SIZE: usize = mem::size_of::<u32>()
        + mem::size_of::<u32>()
        + mem::size_of::<u64>()
        + mem::size_of::<u64>();
    /// Magic
    pub const MAGIC: Magic = Magic::DATA_DESCRIPTOR_SIGNATURE;

    /// Parse the zip64 data descriptor
    pub fn parse(buff: &[u8; Self::SIZE]) -> Result<Self, &'static str> {
        let magic = Magic::from_le_bytes([buff[0], buff[1], buff[2], buff[3]]);
        if magic != Self::MAGIC {
            return Err("Invalid zip64 data descriptor header");
        }
        let crc32 = u32::from_le_bytes([buff[4], buff[5], buff[6], buff[7]]);
        let compressed_size = u64::from_le_bytes([
            buff[8], buff[9], buff[10], buff[11], buff[12], buff[13], buff[14], buff[15],
        ]);
        let uncompressed_size = u64::from_le_bytes([
            buff[16], buff[17], buff[18], buff[19], buff[20], buff[21], buff[22], buff[23],
        ]);

        Ok(Self {
            crc32,
            compressed_size,
            uncompressed_size,
        })
    }

    /// Write the zip64 data descriptor
    pub fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(&Self::MAGIC.to_le_bytes())?;
        w.write_all(&self.crc32.to_le_bytes())?;
        w.write_all(&self.compressed_size.to_le_bytes())?;
        w.write_all(&self.uncompressed_size.to_le_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::format::data_descriptor::Zip64DataDescriptorBlock;
    use crate::format::data_descriptor::ZipDataDescriptorBlock;

    #[test]
    fn test_data_descriptor_wrong_sig() {
        assert_eq!(
            ZipDataDescriptorBlock::MAGIC.to_le_bytes(),
            [0x50, 0x4b, 0x07, 0x08]
        );
        let data_desc =
            ZipDataDescriptorBlock::parse(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
        assert!(data_desc.is_err(),);

        // zip64
        assert_eq!(
            Zip64DataDescriptorBlock::MAGIC.to_le_bytes(),
            [0x50, 0x4b, 0x07, 0x08]
        );
        let data_desc = Zip64DataDescriptorBlock::parse(&[
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
        ]);
        assert!(data_desc.is_err(),);
    }

    #[test]
    fn test_data_descriptor_parsing() {
        let raw = [
            0x50, 0x4b, 0x07, 0x08, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
        ];
        let data_desc = ZipDataDescriptorBlock::parse(&raw).unwrap();
        assert_eq!(
            data_desc,
            ZipDataDescriptorBlock {
                crc32: u32::from_le_bytes(raw[4..8].try_into().unwrap()),
                compressed_size: u32::from_le_bytes(raw[8..12].try_into().unwrap()),
                uncompressed_size: u32::from_le_bytes(raw[12..16].try_into().unwrap()),
            }
        );
        let raw = [
            0x50, 0x4b, 0x07, 0x08, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
            21, 22, 23,
        ];
        let data_desc = Zip64DataDescriptorBlock::parse(&raw).unwrap();
        assert_eq!(
            data_desc,
            Zip64DataDescriptorBlock {
                crc32: u32::from_le_bytes(raw[4..8].try_into().unwrap()),
                compressed_size: u64::from_le_bytes(raw[8..16].try_into().unwrap()),
                uncompressed_size: u64::from_le_bytes(raw[16..24].try_into().unwrap()),
            }
        );
    }
}
