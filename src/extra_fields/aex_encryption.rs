//! AE-x encryption structure extra field

use crate::AesMode;
use crate::CompressionMethod;
use crate::extra_fields::UsedExtraField;
use crate::result::ZipError;
use crate::spec::FixedSizeBlock;
use crate::spec::Pod;
use crate::to_and_from_le;
use crate::types::AesVendorVersion;
use crate::{from_le, to_le};

#[derive(Copy, Clone)]
#[repr(packed, C)]
pub(crate) struct AesExtraField {
    header_id: u16,
    data_size: u16,
    version: u16,
    vendor_id: u16,
    aes_mode: u8,
    compression_method: u16,
}

unsafe impl Pod for AesExtraField {}

impl FixedSizeBlock for AesExtraField {
    type Magic = u16;
    const MAGIC: Self::Magic = UsedExtraField::AeXEncryption.as_u16();

    fn magic(self) -> Self::Magic {
        Self::MAGIC
    }

    const WRONG_MAGIC_ERROR: ZipError =
        ZipError::InvalidArchive(std::borrow::Cow::Borrowed("Wrong AES header ID"));

    to_and_from_le![
        (header_id, u16),
        (data_size, u16),
        (version, u16),
        (vendor_id, u16),
        (aes_mode, u8),
        (compression_method, u16)
    ];
}

impl AesExtraField {
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
