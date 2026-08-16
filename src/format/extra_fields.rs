//! Extra fields

#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(unused)]
pub enum ExtraFieldId {
    Zip64ExtendedInfo = 0x0001,
    AvInfo = 0x0007,                           // AV Info
    ReservedExtendedLanguageEncoding = 0x0008, // Reserved for extended language encoding data (PFS)
    Os2 = 0x0009,                              // OS/2
    Ntfs = 0x000a,
    OpenVms = 0x000c,                                  // OpenVMS
    Unix = 0x000d,                                     // UNIX
    ReservedFileStreamAndForkDescriptors = 0x000e, // Reserved for file stream and fork descriptors
    PatchDescriptor = 0x000f,                      // Patch Descriptor
    Pkcs7StoreForX509Certificates = 0x0014,        // PKCS#7 Store for X.509 Certificates
    X509CertificateIdAndSignature = 0x0015, // X.509 Certificate ID and Signature for individual file
    X509CertificateIdCentralDirectory = 0x0016, // X.509 Certificate ID for Central Directory
    StrongEncryptionHeader = 0x0017,        // Strong Encryption Header
    RecordManagementControls = 0x0018,      // Record Management Controls
    Pkcs7EncryptionRecipientCertificateList = 0x0019, // PKCS#7 Encryption Recipient Certificate List
    ReservedTimestampRecord = 0x0020,                 // Reserved for Timestamp record
    PolicyDecryptionKeyRecord = 0x0021,               // Policy Decryption Key Record
    SmartcryptKeyProviderRecord = 0x0022,             // Smartcrypt Key Provider Record
    SmartcryptPolicyKeyDataRecord = 0x0023,           // Smartcrypt Policy Key Data Record
    IbmS390As400Attributes = 0x0065, // IBM S/390 (Z390), AS/400 (I400) attributes - uncompressed
    ReservedIbmS390As400AttributesCompressed = 0x0066, // Reserved for IBM S/390 (Z390), AS/400 (I400) attributes - compressed
    // Third party mappings commonly used
    Macintosh = 0x07c8,                   // Macintosh
    PixarUsdHeader = 0x1986,              // Pixar USD header ID
    ZipItMacintosh = 0x2605,              // ZipIt Macintosh
    ZipItMacintosh135Plus = 0x2705,       // ZipIt Macintosh 1.3.5+
    ZipItMacintosh135PlusAlt = 0x2805,    // ZipIt Macintosh 1.3.5+
    InfoZipMacintosh = 0x334d,            // Info-ZIP Macintosh
    Tandem = 0x4154,                      // Tandem
    AcornSparkFs = 0x4341,                // Acorn/SparkFS
    WindowsNtSecurityDescriptor = 0x4453, // Windows NT security descriptor (binary ACL)
    Poszip4690 = 0x4690,                  // POSZIP 4690 (reserved)
    VmCms = 0x4704,                       // VM/CMS
    Mvs = 0x470f,                         // MVS
    TheosOld = 0x4854,                    // THEOS (old?)
    FwkcsMd5 = 0x4b46,                    // FWKCS MD5
    Os2AccessControlList = 0x4c41,        // OS/2 access control list (text ACL)
    InfoZipOpenVms = 0x4d49,              // Info-ZIP OpenVMS
    MacintoshSmartzip = 0x4d63,           // Macintosh Smartzip (??)
    XceedOriginalLocation = 0x4f4c,       // Xceed original location extra field
    AosVsAcl = 0x5356,                    // AOS/VS (ACL)
    ExtendedTimestamp = 0x5455,
    XceedUnicode = 0x554e,        // Xceed unicode extra field
    InfoZipUnixOriginal = 0x5855, // Info-ZIP UNIX (original, also OS/2, NT, etc)
    UnicodeComment = 0x6375,
    BeOsBeBox = 0x6542, // BeOS/BeBox
    Theos = 0x6854,     // THEOS
    UnicodePath = 0x7075,
    AtheosSyllable = 0x7441,         // AtheOS/Syllable
    AsiUnix = 0x756e,                // ASi UNIX
    InfoZipUnixNew = 0x7855,         // Info-ZIP UNIX (new)
    InfoZipUnixNewerUidGid = 0x7875, // Info-ZIP UNIX (newer UID/GID)
    AeXEncryption = 0x9901,
    Unknown9902 = 0x9902, // unknown
    DataStreamAlignment = 0xa11e,
    MicrosoftOpenPackagingGrowthHint = 0xa220, // Microsoft Open Packaging Growth Hint
    JavaJar = 0xcafe,                          // Java JAR file Extra Field Header ID
    AndroidZipAlignment = 0xd935,              // Android ZIP Alignment Extra Field
    KoreanZipCodePageInfo = 0xe57a,            // Korean ZIP code page info
    SmsQdos = 0xfd4a,                          // SMS/QDOS
}

impl ExtraFieldId {
    pub const fn as_u16(self) -> u16 {
        self as u16
    }
}

/// Known Extra fields (PKWARE and Third party) mappings, sorted
pub const EXTRA_FIELD_MAPPING: [u16; 59] = [
    ExtraFieldId::Zip64ExtendedInfo.as_u16(),
    ExtraFieldId::AvInfo.as_u16(),
    ExtraFieldId::ReservedExtendedLanguageEncoding.as_u16(),
    ExtraFieldId::Os2.as_u16(),
    ExtraFieldId::Ntfs.as_u16(),
    ExtraFieldId::OpenVms.as_u16(),
    ExtraFieldId::Unix.as_u16(),
    ExtraFieldId::ReservedFileStreamAndForkDescriptors.as_u16(),
    ExtraFieldId::PatchDescriptor.as_u16(),
    ExtraFieldId::Pkcs7StoreForX509Certificates.as_u16(),
    ExtraFieldId::X509CertificateIdAndSignature.as_u16(),
    ExtraFieldId::X509CertificateIdCentralDirectory.as_u16(),
    ExtraFieldId::StrongEncryptionHeader.as_u16(),
    ExtraFieldId::RecordManagementControls.as_u16(),
    ExtraFieldId::Pkcs7EncryptionRecipientCertificateList.as_u16(),
    ExtraFieldId::ReservedTimestampRecord.as_u16(),
    ExtraFieldId::PolicyDecryptionKeyRecord.as_u16(),
    ExtraFieldId::SmartcryptKeyProviderRecord.as_u16(),
    ExtraFieldId::SmartcryptPolicyKeyDataRecord.as_u16(),
    ExtraFieldId::IbmS390As400Attributes.as_u16(),
    ExtraFieldId::ReservedIbmS390As400AttributesCompressed.as_u16(),
    // Third party mappings commonly used
    ExtraFieldId::Macintosh.as_u16(),
    ExtraFieldId::PixarUsdHeader.as_u16(),
    ExtraFieldId::ZipItMacintosh.as_u16(),
    ExtraFieldId::ZipItMacintosh135Plus.as_u16(),
    ExtraFieldId::ZipItMacintosh135PlusAlt.as_u16(),
    ExtraFieldId::InfoZipMacintosh.as_u16(),
    ExtraFieldId::Tandem.as_u16(),
    ExtraFieldId::AcornSparkFs.as_u16(),
    ExtraFieldId::WindowsNtSecurityDescriptor.as_u16(),
    ExtraFieldId::Poszip4690.as_u16(),
    ExtraFieldId::VmCms.as_u16(),
    ExtraFieldId::Mvs.as_u16(),
    ExtraFieldId::TheosOld.as_u16(),
    ExtraFieldId::FwkcsMd5.as_u16(),
    ExtraFieldId::Os2AccessControlList.as_u16(),
    ExtraFieldId::InfoZipOpenVms.as_u16(),
    ExtraFieldId::MacintoshSmartzip.as_u16(),
    ExtraFieldId::XceedOriginalLocation.as_u16(),
    ExtraFieldId::AosVsAcl.as_u16(),
    ExtraFieldId::ExtendedTimestamp.as_u16(),
    ExtraFieldId::XceedUnicode.as_u16(),
    ExtraFieldId::InfoZipUnixOriginal.as_u16(),
    ExtraFieldId::UnicodeComment.as_u16(),
    ExtraFieldId::BeOsBeBox.as_u16(),
    ExtraFieldId::Theos.as_u16(),
    ExtraFieldId::UnicodePath.as_u16(),
    ExtraFieldId::AtheosSyllable.as_u16(),
    ExtraFieldId::AsiUnix.as_u16(),
    ExtraFieldId::InfoZipUnixNew.as_u16(),
    ExtraFieldId::InfoZipUnixNewerUidGid.as_u16(),
    ExtraFieldId::AeXEncryption.as_u16(),
    ExtraFieldId::Unknown9902.as_u16(),
    ExtraFieldId::DataStreamAlignment.as_u16(),
    ExtraFieldId::MicrosoftOpenPackagingGrowthHint.as_u16(),
    ExtraFieldId::JavaJar.as_u16(),
    ExtraFieldId::AndroidZipAlignment.as_u16(),
    ExtraFieldId::KoreanZipCodePageInfo.as_u16(),
    ExtraFieldId::SmsQdos.as_u16(),
];
