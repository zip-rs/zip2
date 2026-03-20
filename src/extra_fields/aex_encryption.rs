//! AE-x encryption structure extra field

use crate::AesMode;
use crate::CompressionMethod;
use crate::extra_fields::UsedExtraField;
use crate::spec::{Pod, to_le};
use crate::types::AesVendorVersion;

#[derive(Copy, Clone)]
#[repr(packed, C)]
pub(crate) struct AexEncryption {
    header_id: u16,
    data_size: u16,
    pub(crate) version: u16,
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

    #[inline]
    pub(crate) fn new(
        version: AesVendorVersion,
        aes_mode: AesMode,
        compression_method: CompressionMethod,
    ) -> Self {
        Self {
            header_id: UsedExtraField::AeXEncryption.as_u16(),
            data_size: (size_of::<u16>() + size_of::<u16>() + size_of::<u8>() + size_of::<u16>())
                as u16,
            version: version.as_u16(),
            vendor_id: u16::from_le_bytes(*b"AE"),
            aes_mode: aes_mode.as_u8(),
            compression_method: compression_method.serialize_to_u16(),
        }
        .to_le()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_create_aex() {
        use super::AexEncryption;
        use crate::AesMode;
        use crate::CompressionMethod;
        use crate::spec::Pod;
        use crate::types::AesVendorVersion;

        let aex_encryption = AexEncryption::new(
            AesVendorVersion::Ae2,
            AesMode::Aes256,
            CompressionMethod::Stored,
        );

        let buf = aex_encryption.as_bytes();
        assert_eq!(buf.len(), 11);
        assert_eq!(buf[0..2], [1, 153]);
        assert_eq!(buf[2..4], [7, 0]);
        assert_eq!(buf[4..6], [2, 0]);
        assert_eq!(buf[6..8], [65, 69]);
        assert_eq!(buf[8], 0x03);
        assert_eq!(buf[9..], [0, 0]);

        // test length used in write.rs
        assert_eq!(buf[std::mem::offset_of!(AexEncryption, version)..].len(), 7);
    }
}
