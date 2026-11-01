//! Extra fields

/// Known Extra Field Id from PKWARE specifications
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(unused)]
#[non_exhaustive]
pub enum ExtraFieldId {
    /// Zip64 Extended information
    Zip64ExtendedInfo = 0x0001,
    /// AV Info
    AvInfo = 0x0007,
    /// Reserved for extended language encoding data (PFS)
    ReservedExtendedLanguageEncoding = 0x0008,
    /// OS/2
    Os2 = 0x0009,
    /// Ntfs
    Ntfs = 0x000a,
    /// OpenVMS
    OpenVms = 0x000c,
    /// UNIX
    Unix = 0x000d,
    /// Reserved for file stream and fork descriptors
    ReservedFileStreamAndForkDescriptors = 0x000e,
    /// Patch Descriptor
    PatchDescriptor = 0x000f,
    /// PKCS#7 Store for X.509 Certificates
    Pkcs7StoreForX509Certificates = 0x0014,
    /// X.509 Certificate ID and Signature for individual file
    X509CertificateIdAndSignature = 0x0015,
    /// X.509 Certificate ID for Central Directory
    X509CertificateIdCentralDirectory = 0x0016,
    /// Strong Encryption Header
    StrongEncryptionHeader = 0x0017,
    /// Record Management Controls
    RecordManagementControls = 0x0018,
    /// PKCS#7 Encryption Recipient Certificate List
    Pkcs7EncryptionRecipientCertificateList = 0x0019,
    /// Reserved for Timestamp record
    ReservedTimestampRecord = 0x0020,
    /// Policy Decryption Key Record
    PolicyDecryptionKeyRecord = 0x0021,
    /// Smartcrypt Key Provider Record
    SmartcryptKeyProviderRecord = 0x0022,
    /// Smartcrypt Policy Key Data Record
    SmartcryptPolicyKeyDataRecord = 0x0023,
    /// IBM S/390 (Z390), AS/400 (I400) attributes - uncompressed
    IbmS390As400Attributes = 0x0065,
    /// Reserved for IBM S/390 (Z390), AS/400 (I400) attributes - compressed
    ReservedIbmS390As400AttributesCompressed = 0x0066,
    // Third party mappings commonly used
    /// Macintosh
    Macintosh = 0x07c8,
    /// Pixar USD header ID
    PixarUsdHeader = 0x1986,
    /// ZipIt Macintosh
    ZipItMacintosh = 0x2605,
    /// ZipIt Macintosh 1.3.5+
    ZipItMacintosh135Plus = 0x2705,
    /// ZipIt Macintosh 1.3.5+
    ZipItMacintosh135PlusAlt = 0x2805,
    /// Info-ZIP Macintosh
    InfoZipMacintosh = 0x334d,
    /// Tandem
    Tandem = 0x4154,
    /// Acorn/SparkFS
    AcornSparkFs = 0x4341,
    /// Windows NT security descriptor (binary ACL)
    WindowsNtSecurityDescriptor = 0x4453,
    /// POSZIP 4690 (reserved)
    Poszip4690 = 0x4690,
    /// VM/CMS
    VmCms = 0x4704,
    /// MVS
    Mvs = 0x470f,
    /// THEOS (old?)
    TheosOld = 0x4854,
    /// FWKCS MD5
    FwkcsMd5 = 0x4b46,
    /// OS/2 access control list (text ACL)
    Os2AccessControlList = 0x4c41,
    /// Info-ZIP OpenVMS
    InfoZipOpenVms = 0x4d49,
    /// Macintosh Smartzip (??)
    MacintoshSmartzip = 0x4d63,
    /// Xceed original location extra field
    XceedOriginalLocation = 0x4f4c,
    /// AOS/VS (ACL)
    AosVsAcl = 0x5356,
    /// Extended Timestamp
    ExtendedTimestamp = 0x5455,
    /// Xceed unicode extra field
    XceedUnicode = 0x554e,
    /// Info-ZIP UNIX (original, also OS/2, NT, etc)
    InfoZipUnixOriginal = 0x5855,
    /// Unicode comment
    UnicodeComment = 0x6375,
    /// BeOS/BeBox
    BeOsBeBox = 0x6542,
    /// THEOS
    Theos = 0x6854,
    /// Unicode Path
    UnicodePath = 0x7075,
    /// AtheOS/Syllable
    AtheosSyllable = 0x7441,
    /// ASi UNIX
    AsiUnix = 0x756e,
    /// Info-ZIP UNIX (new)
    InfoZipUnixNew = 0x7855,
    /// Info-ZIP UNIX (newer UID/GID)
    InfoZipUnixNewerUidGid = 0x7875,
    /// AE-X Encryption
    AeXEncryption = 0x9901,
    /// unknown
    Unknown9902 = 0x9902,
    /// DataStream Alignment
    DataStreamAlignment = 0xa11e,
    /// Microsoft Open Packaging Growth Hint
    MicrosoftOpenPackagingGrowthHint = 0xa220,
    /// Java JAR file Extra Field Header ID
    JavaJar = 0xcafe,
    /// Android ZIP Alignment Extra Field
    AndroidZipAlignment = 0xd935,
    /// Korean ZIP code page info
    KoreanZipCodePageInfo = 0xe57a,
    /// SMS/QDOS
    SmsQdos = 0xfd4a,
}

impl ExtraFieldId {
    /// Get value as u16
    #[must_use]
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
