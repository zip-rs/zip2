//! AE-x encryption structure extra field

use std::io::{ErrorKind, Read, Write};

use crate::AesMode;
use crate::CompressionMethod;
use crate::extra_fields::UsedExtraField;
use crate::result::{ZipError, ZipResult, invalid, invalid_archive_const};
use crate::types::AesVendorVersion;
use crate::unstable::LittleEndianReadExt;

#[derive(Copy, Clone, Debug)]
pub struct AexEncryption {
    pub(crate) aes_vendor_version: AesVendorVersion,
    pub(crate) aes_mode: AesMode,
    pub(crate) compression_method: CompressionMethod,
    pub(crate) aes_extra_field_start: Option<usize>,
}

impl AexEncryption {
    /// Field Header ID
    pub(crate) const EXTRA_FIELD_ID: u16 = UsedExtraField::AeXEncryption.as_u16();
    /// Field size
    pub(crate) const EXTRA_FIELD_SIZE: u16 =
        (size_of::<u16>() + size_of::<u16>() + size_of::<u8>() + size_of::<u16>()) as u16;
    /// 0x4541
    pub(crate) const VENDOR_ID: u16 = u16::from_le_bytes(*b"AE");
    /// Full size of the extra field
    pub(crate) const FULL_SIZE: usize =
        size_of::<u16>() + size_of::<u16>() + Self::EXTRA_FIELD_SIZE as usize;

    #[inline]
    pub(crate) fn new(
        aes_vendor_version: AesVendorVersion,
        aes_mode: AesMode,
        compression_method: CompressionMethod,
    ) -> Self {
        Self {
            aes_vendor_version,
            aes_mode,
            compression_method,
            aes_extra_field_start: None,
        }
    }

    pub fn write<T: Write>(self, writer: &mut T) -> ZipResult<()> {
        writer.write_all(&u16::to_le_bytes(Self::EXTRA_FIELD_ID))?;
        writer.write_all(&u16::to_le_bytes(Self::EXTRA_FIELD_SIZE))?;
        self.write_data(writer)?;
        Ok(())
    }

    pub fn write_data<T: Write>(self, writer: &mut T) -> ZipResult<()> {
        writer.write_all(&self.aes_vendor_version.as_u16().to_le_bytes())?;
        writer.write_all(&u16::to_le_bytes(Self::VENDOR_ID))?;
        writer.write_all(&self.aes_mode.as_u8().to_le_bytes())?;
        writer.write_all(&self.compression_method.serialize_to_u16().to_le_bytes())?;
        Ok(())
    }

    #[inline]
    pub(crate) fn parse<R: Read>(
        reader: &mut R,
        len: u16,
    ) -> ZipResult<((AesMode, AesVendorVersion), CompressionMethod)> {
        if len != 7 {
            return Err(ZipError::UnsupportedArchive(
                "AES extra data field has an unsupported length",
            ));
        }
        let vendor_version = reader.read_u16_le()?;
        let vendor_id = reader.read_u16_le()?;
        let mut buff = [0u8];
        if let Err(e) = reader.read_exact(&mut buff) {
            if e.kind() == ErrorKind::UnexpectedEof {
                return Err(invalid!("AES extra field truncated"));
            }
            return Err(e.into());
        }
        if vendor_id != Self::VENDOR_ID {
            return Err(invalid!("Invalid AES vendor"));
        }
        let vendor_version = vendor_version.try_into().map_err(invalid_archive_const)?;
        let aes_mode = buff[0].try_into().map_err(invalid_archive_const)?;
        let inner_comp_method = CompressionMethod::parse_from_u16(reader.read_u16_le()?);
        Ok(((aes_mode, vendor_version), inner_comp_method))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_create_aex() {
        use super::AexEncryption;
        use crate::AesMode;
        use crate::CompressionMethod;
        use crate::types::AesVendorVersion;

        let aex_encryption = AexEncryption::new(
            AesVendorVersion::Ae2,
            AesMode::Aes256,
            CompressionMethod::Stored,
        );
        let mut buf = Vec::new();
        aex_encryption.write(&mut buf).unwrap();

        assert_eq!(buf.len(), 11);
        assert_eq!(buf[0..2], [1, 153]);
        assert_eq!(buf[2..4], [7, 0]);
        assert_eq!(buf[4..6], [2, 0]);
        assert_eq!(buf[6..8], [65, 69]);
        assert_eq!(buf[8], 0x03);
        assert_eq!(buf[9..], [0, 0]);
    }

    #[test]
    fn test_too_long_length() {
        use super::AexEncryption;
        use std::io::Cursor;

        let data = &[0, 1, 2, 3, 4, 5, 6, 7];
        let len = data.len() as u16;
        let mut cursor = Cursor::new(data);

        let res = AexEncryption::parse(&mut cursor, len);
        assert!(res.is_err());
    }

    #[test]
    fn test_serialize_parse() {
        use super::AexEncryption;
        use crate::AesMode;
        use crate::CompressionMethod;
        use crate::types::AesVendorVersion;
        use std::io::Cursor;

        let aex_encryption = AexEncryption::new(
            AesVendorVersion::Ae2,
            AesMode::Aes256,
            CompressionMethod::Stored,
        );

        let mut data = Vec::new();
        aex_encryption.write(&mut data).unwrap();

        let len_data = u16::from_le_bytes([data[2], data[3]]);
        let data = &data[4..]; // remove the signature
        let len = data.len() as u16;
        assert_eq!(len_data, len);
        assert_eq!(len, 7);
        let mut cursor = Cursor::new(data);

        let res = AexEncryption::parse(&mut cursor, len);
        assert!(res.is_ok());
        let (aes_mode_options, inner_compression_method) = res.unwrap();
        assert_eq!(aes_mode_options, (AesMode::Aes256, AesVendorVersion::Ae2));
        assert_eq!(inner_compression_method, CompressionMethod::Stored);
    }
}
