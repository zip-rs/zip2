//! AE-x encryption structure extra field

use crate::AesMode;
use crate::CompressionMethod;
use crate::extra_fields::UsedExtraField;
use crate::spec::Pod;
use crate::to_le;
use crate::types::AesVendorVersion;

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

unsafe impl Pod for AexEncryption {}

impl AexEncryption {
    #[inline(always)]
    pub(crate) fn to_le(mut self) -> Self {
        to_le![
            self,
            [
                (header_id, u16),
                (data_size, u16),
                (version, u16),
                (vendor_id, u16),
                (aes_mode, u8),
                (compression_method, u16)
            ]
        ];
        self
    }

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
