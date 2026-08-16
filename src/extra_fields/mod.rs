//! Types for extra fields

use core::fmt::Display;

mod aex_encryption;
mod data_stream_alignment;
mod extended_timestamp;
mod extra_field;
mod ntfs;
mod zip64_extended_information;
mod custom_extra_field;
mod zipinfo_utf8;

// re-export extra fields
#[cfg(feature = "aes-crypto")]
pub use aex_encryption::AexEncryption;
pub use data_stream_alignment::DataStreamAlignment;
pub use extended_timestamp::ExtendedTimestamp;
pub use extra_field::{ExtraField, ExtraFields};
pub use ntfs::Ntfs;
pub use zip64_extended_information::{Zip64ExtendedInformation, Zip64Sizes};
pub use zipinfo_utf8::UnicodeExtraField;
pub use custom_extra_field::CustomExtraField;

// re-export
pub use crate::format::extra_fields::EXTRA_FIELD_MAPPING;

/// Marker trait to denote the place where this extra field has been stored.
pub trait ExtraFieldVersion {}

/// Marker type for extra fields specified in a local file header.
#[derive(Debug, Clone)]
pub struct LocalHeaderVersion;

/// Use this marker type for extra fields specified in the central header.
#[derive(Debug, Clone)]
pub struct CentralHeaderVersion;

impl ExtraFieldVersion for LocalHeaderVersion {}
impl ExtraFieldVersion for CentralHeaderVersion {}

/// Internal extra-field identifiers (`u16` tags) recognized by this crate.
///
/// This enum is crate-private and used for matching/dispatch on raw ZIP extra
/// field IDs. It is distinct from [`ExtraField`], which represents parsed,
/// public extra-field data structures.
#[repr(u16)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum UsedExtraField {
    /// ZIP64 extended information extra field
    Zip64ExtendedInfo = 0x0001,
    /// NTFS
    Ntfs = 0x000a,
    /// extended timestamp
    /// from <https://libzip.org/specifications/extrafld.txt>
    ExtendedTimestamp = 0x5455,
    /// Info-ZIP Unicode Comment Extra Field
    UnicodeComment = 0x6375,
    /// Info-ZIP Unicode Path Extra Field
    UnicodePath = 0x7075,
    /// AE-x encryption structure
    AeXEncryption = 0x9901,
    /// Data Stream Alignment (Apache Commons-Compress)
    DataStreamAlignment = 0xa11e,
}

impl UsedExtraField {
    pub const fn to_le_bytes(self) -> [u8; 2] {
        let field_u16 = self.as_u16();
        field_u16.to_le_bytes()
    }

    pub const fn as_u16(self) -> u16 {
        self as u16
    }
}

impl From<UsedExtraField> for u16 {
    fn from(value: UsedExtraField) -> Self {
        value.as_u16()
    }
}

impl Display for UsedExtraField {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "0x{:04X}", *self as u16)
    }
}

macro_rules! extra_field_match {
    ($x:expr, $( $variant:path ),+ $(,)?) => {
        match $x {
            $(
                v if v == $variant as u16 => Ok($variant),
            )+
            _ => Err(()),
        }
    };
}

impl TryFrom<u16> for UsedExtraField {
    type Error = ();

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        extra_field_match!(
            value,
            UsedExtraField::Zip64ExtendedInfo,
            UsedExtraField::Ntfs,
            UsedExtraField::ExtendedTimestamp,
            UsedExtraField::UnicodeComment,
            UsedExtraField::UnicodePath,
            UsedExtraField::DataStreamAlignment,
            UsedExtraField::AeXEncryption,
        )
    }
}
