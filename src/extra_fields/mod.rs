//! Types for extra fields

/// marker trait to denote the place where this extra field has been stored
pub trait ExtraFieldVersion {}

/// use this to mark extra fields specified in a local header

#[derive(Debug, Clone)]
pub struct LocalHeaderVersion;

/// use this to mark extra fields specified in the central header

#[derive(Debug, Clone)]
pub struct CentralHeaderVersion;

impl ExtraFieldVersion for LocalHeaderVersion {}
impl ExtraFieldVersion for CentralHeaderVersion {}

mod extended_timestamp;
mod ntfs;
mod zipinfo_utf8;

// re-export
pub use extended_timestamp::*;
pub use ntfs::Ntfs;
pub use zipinfo_utf8::UnicodeExtraField;

/// contains one extra field
#[derive(Debug, Clone)]
pub enum ExtraField {
    /// NTFS extra field
    Ntfs(Ntfs),

    /// extended timestamp, as described in <https://libzip.org/specifications/extrafld.txt>
    ExtendedTimestamp(ExtendedTimestamp),
}

/// Known Extra fields (PKWARE and Third party) mappings
pub const EXTRA_FIELD_MAPPING: [u16; 57] = [
    0x0007, // AV Info
    0x0008, // Reserved for extended language encoding data (PFS)
    0x0009, // OS/2
    0x000a, // NTFS
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
    0x4690, // POSZIP 4690 (reserved)
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
    0x4704, // VM/CMS
    0x470f, // MVS
    0x4854, // THEOS (old?)
    0x4b46, // FWKCS MD5
    0x4c41, // OS/2 access control list (text ACL)
    0x4d49, // Info-ZIP OpenVMS
    0x4d63, // Macintosh Smartzip (??)
    0x4f4c, // Xceed original location extra field
    0x5356, // AOS/VS (ACL)
    0x5855, // extended timestamp
    0x554e, // Xceed unicode extra field
    0x6375, // Info-ZIP Unicode Comment Extra Field
    0x6542, // BeOS/BeBox
    0x6854, // THEOS
    0x7075, // Info-ZIP Unicode Path Extra Field
    0x7441, // AtheOS/Syllable
    0x756e, // ASi UNIX
    0x7855, // Info-ZIP UNIX (new)
    0x7875, // Info-ZIP UNIX (newer UID/GID)
    0xa11e, // Data Stream Alignment (Apache Commons-Compress)
    0xa220, // Microsoft Open Packaging Growth Hint
    0xcafe, // Java JAR file Extra Field Header ID
    0xd935, // Android ZIP Alignment Extra Field
    0xe57a, // Korean ZIP code page info
    0xfd4a, // SMS/QDOS
    0x9901, // AE-x encryption structure
    0x9902, // unknown
];
