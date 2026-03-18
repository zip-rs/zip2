//! AE-x encryption structure extra field

use crate::AesMode;
use crate::CompressionMethod;
use crate::extra_fields::UsedExtraField;
use crate::result::ZipError;
use crate::spec::Pod;
use crate::to_and_from_le;
use crate::types::AesVendorVersion;
use crate::{from_le, to_le};
use std::io::Write;

#[derive(Copy, Clone)]
#[repr(packed, C)]
pub(crate) struct AexEncryption {
    header_id: u16,
    data_size: u16,
    version: u16,
    vendor_id: u16,
    aes_mode: u8,
    compression_method: u16,
}

unsafe impl Pod for AexEncryption{}

impl AexEncryption {

    pub(crate) fn write<T: Write + ?Sized>(self, writer: &mut T) -> ZipResult<()> {
        let block = self.to_le();
        writer.write_all(block.as_bytes())?;
        Ok(())
    }

    to_and_from_le![
        (header_id, u16),
        (data_size, u16),
        (version, u16),
        (vendor_id, u16),
        (aes_mode, u8),
        (compression_method, u16)
    ];
}

impl AexEncryption {
    pub(crate) fn new(
        version: AesVendorVersion,
        aes_mode: AesMode,
        compression_method: CompressionMethod,
    ) -> Self {
        Self {
            header_id: UsedExtraField::AeXEncryption.as_u16(),
            data_size: 7,
            version: version as u16,
            vendor_id: u16::from_le_bytes(*b"AE"),
            aes_mode: aes_mode as u8,
            compression_method: compression_method.serialize_to_u16(),
        }
    }
}
