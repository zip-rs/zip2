//! Types for extra fields

use crate::result::ZipResult;
use crate::result::invalid;
use core::fmt::Display;

mod aex_encryption;
mod extended_timestamp;
mod ntfs;
mod zip64_extended_information;
mod zipinfo_utf8;

pub(crate) use aex_encryption::AexEncryption;
pub(crate) use zip64_extended_information::Zip64ExtendedInformation;

// re-export
pub use extended_timestamp::ExtendedTimestamp;
pub use ntfs::Ntfs;
pub use zipinfo_utf8::UnicodeExtraField;

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

/// Contains one extra field.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ExtraField {
    /// NTFS extra field
    Ntfs(Ntfs),

    /// extended timestamp, as described in <https://libzip.org/specifications/extrafld.txt>
    ExtendedTimestamp(ExtendedTimestamp),
}

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

/// Known Extra fields (PKWARE and Third party) mappings, sorted
pub const EXTRA_FIELD_MAPPING: [u16; 59] = [
    UsedExtraField::Zip64ExtendedInfo.as_u16(),
    0x0007, // AV Info
    0x0008, // Reserved for extended language encoding data (PFS)
    0x0009, // OS/2
    UsedExtraField::Ntfs.as_u16(),
    0x000c, // OpenVMS
    0x000d, // UNIX
    0x000e, // Reserved for file stream and fork descriptors
    0x000f, // Patch Descriptor
    0x0014, // PKCS#7 Store for X.509 Certificates
    0x0015, // X.509 Certificate ID and Signature for individual file
    0x0016, // X.509 Certificate ID for Central Directory
    0x0017, // Strong Encryption Header
    0x0018, // Record Management Controls
    0x0019, // PKCS#7 Encryption Recipient Certificate List
    0x0020, // Reserved for Timestamp record
    0x0021, // Policy Decryption Key Record
    0x0022, // Smartcrypt Key Provider Record
    0x0023, // Smartcrypt Policy Key Data Record
    0x0065, // IBM S/390 (Z390), AS/400 (I400) attributes - uncompressed
    0x0066, // Reserved for IBM S/390 (Z390), AS/400 (I400) attributes - compressed
    // Third party mappings commonly used
    0x07c8, // Macintosh
    0x1986, // Pixar USD header ID
    0x2605, // ZipIt Macintosh
    0x2705, // ZipIt Macintosh 1.3.5+
    0x2805, // ZipIt Macintosh 1.3.5+
    0x334d, // Info-ZIP Macintosh
    0x4154, // Tandem
    0x4341, // Acorn/SparkFS
    0x4453, // Windows NT security descriptor (binary ACL)
    0x4690, // POSZIP 4690 (reserved)
    0x4704, // VM/CMS
    0x470f, // MVS
    0x4854, // THEOS (old?)
    0x4b46, // FWKCS MD5
    0x4c41, // OS/2 access control list (text ACL)
    0x4d49, // Info-ZIP OpenVMS
    0x4d63, // Macintosh Smartzip (??)
    0x4f4c, // Xceed original location extra field
    0x5356, // AOS/VS (ACL)
    UsedExtraField::ExtendedTimestamp.as_u16(),
    0x554e, // Xceed unicode extra field
    0x5855, // Info-ZIP UNIX (original, also OS/2, NT, etc)
    UsedExtraField::UnicodeComment.as_u16(),
    0x6542, // BeOS/BeBox
    0x6854, // THEOS
    UsedExtraField::UnicodePath.as_u16(),
    0x7441, // AtheOS/Syllable
    0x756e, // ASi UNIX
    0x7855, // Info-ZIP UNIX (new)
    0x7875, // Info-ZIP UNIX (newer UID/GID)
    UsedExtraField::AeXEncryption.as_u16(),
    0x9902, // unknown
    UsedExtraField::DataStreamAlignment.as_u16(),
    0xa220, // Microsoft Open Packaging Growth Hint
    0xcafe, // Java JAR file Extra Field Header ID
    0xd935, // Android ZIP Alignment Extra Field
    0xe57a, // Korean ZIP code page info
    0xfd4a, // SMS/QDOS
];

/// A Custom Extra Field
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CustomExtraField {
    /// If true, this field will be included in the central directory entry but not the local file header.
    pub(crate) central_only: bool,
    /// Header ID of the extra field
    header_id: u16,
    /// Data of the extra field
    data: Box<[u8]>,
}

impl CustomExtraField {
    pub(crate) fn new(central_only: bool, header_id: u16, data: &[u8]) -> Self {
        Self {
            central_only,
            header_id,
            data: data.into(),
        }
    }

    #[allow(unused)] // used for tests
    pub(crate) fn new_from_raw(central_only: bool, data: &[u8]) -> ZipResult<Self> {
        if data.len() < 2 {
            return Err(invalid!("Cannot build a CustomExtraField: no header_id"));
        }
        if data.len() < 4 {
            return Err(invalid!("Cannot build a CustomExtraField: no size"));
        }
        let header_id = u16::from_le_bytes([data[0], data[1]]);
        let size = u16::from_le_bytes([data[2], data[3]]) as usize;
        if size > (u16::MAX - 4) as usize {
            return Err(invalid!("Cannot build a CustomExtraField: size too big"));
        }
        let data_rest = &data[4..];
        if size != data_rest.len() {
            return Err(invalid!("Cannot build a CustomExtraField: incorrect size"));
        }
        Ok(Self {
            central_only,
            header_id,
            data: data_rest.to_vec().into_boxed_slice(),
        })
    }

    pub(crate) fn len_with_header(&self) -> usize {
        let size = self.data.len();
        size_of::<u16>() + size_of::<u16>() + size
    }

    pub(crate) fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.data.len());

        out.extend_from_slice(&self.header_id.to_le_bytes());
        let size = self.data.len() as u16;
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&self.data);

        out
    }
}

#[cfg(feature = "_arbitrary")]
impl arbitrary::Arbitrary<'_> for CustomExtraField {
    fn arbitrary(u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<Self> {
        Ok(CustomExtraField {
            central_only: u.arbitrary()?,
            header_id: u.arbitrary()?,
            data: u.arbitrary()?,
        })
    }
}
