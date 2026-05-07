//! Flags of zip

/// System inside `version made by` (upper byte)
/// Reference: 4.4.2.2
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(clippy::upper_case_acronyms)]
#[repr(u8)]
pub enum System {
    /// `MS-DOS` and `OS/2` (`FAT` / `VFAT` / `FAT32` file systems; default on Windows)
    Dos = 0,
    /// `Amiga`
    Amiga = 1,
    /// `OpenVMS`
    OpenVMS = 2,
    /// Default on Unix; default for symlinks on all platforms
    Unix = 3,
    /// `VM/CMS`
    VmCms = 4,
    /// `Atari ST`
    AtariSt = 5,
    /// `OS/2 H.P.F.S.`
    Os2 = 6,
    /// Legacy `Mac OS`, pre `OS X`
    Macintosh = 7,
    /// `Z-System`
    ZSystem = 8,
    /// `CP/M`
    CPM = 9,
    /// Windows NTFS (with extra attributes; not used by default)
    WindowsNTFS = 10,
    /// `MVS (OS/390 - Z/OS)`
    MVS = 11,
    /// `VSE`
    VSE = 12,
    /// `Acorn Risc`
    AcornRisc = 13,
    /// `VFAT`
    VFAT = 14,
    /// alternate MVS
    AlternateMVS = 15,
    /// `BeOS`
    BeOS = 16,
    /// `Tandem`
    Tandem = 17,
    /// `OS/400`
    Os400 = 18,
    /// `OS X` (Darwin) (with extra attributes; not used by default)
    OsDarwin = 19,
    /// unused
    #[default]
    Unknown = 255,
}

impl System {
    /// Parse `version_made_by` block in local entry block.
    #[must_use]
    pub fn from_version_made_by(version_made_by: u16) -> Self {
        // Extract upper byte from little-endian representation
        let upper_byte = version_made_by.to_le_bytes()[1];
        System::from(upper_byte) // from u8
    }

    /// Extract the system and version from a `version_made_by` field.
    /// The first byte (lower) is the version, and the second byte (upper) is the system.
    pub(crate) fn extract_bytes(version_made_by: u16) -> (u8, Self) {
        let bytes = version_made_by.to_le_bytes();
        (bytes[0], Self::from(bytes[1]))
    }
}

impl From<u8> for System {
    fn from(system: u8) -> Self {
        match system {
            0 => System::Dos,
            1 => System::Amiga,
            2 => System::OpenVMS,
            3 => System::Unix,
            4 => System::VmCms,
            5 => System::AtariSt,
            6 => System::Os2,
            7 => System::Macintosh,
            8 => System::ZSystem,
            9 => System::CPM,
            10 => System::WindowsNTFS,
            11 => System::MVS,
            12 => System::VSE,
            13 => System::AcornRisc,
            14 => System::VFAT,
            15 => System::AlternateMVS,
            16 => System::BeOS,
            17 => System::Tandem,
            18 => System::Os400,
            19 => System::OsDarwin,
            _ => System::Unknown,
        }
    }
}

impl From<System> for u8 {
    fn from(system: System) -> Self {
        system as u8
    }
}

/// Zip flags
/// Stored as Little endian
#[allow(unused)]
#[rustfmt::skip]
#[repr(u16)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum ZipFlags {
    /// If set, indicates that the file is encrypted.
    Encrypted                   = 0b0000_0000_0000_0001,
    CompressionSetting          = 0b0000_0000_0000_0010,
    CompressionSetting2         = 0b0000_0000_0000_0100,
    /// If this bit is set, the fields crc-32, compressed size and uncompressed size are set to zero in the  local header.
    /// The correct values are put in the data descriptor immediately following the compressed data.
    UsingDataDescriptor         = 0b0000_0000_0000_1000,
    /// Reserved for use with method 8, for enhanced deflating.
    ReservedEnhancedDeflating   = 0b0000_0000_0001_0000,
    /// If this bit is set, this indicates that the file is compressed patched data.
    CompressedPatchedData       = 0b0000_0000_0010_0000,
    /// Strong encryption.
    /// If this bit is set, you MUST set the version needed to extract value to at least 50 and you MUST also set bit 0.
    /// If AES encryption is used, the version needed to extract value MUST be at least 51.
    StrongEncryption            = 0b0000_0000_0100_0000,
    // bit 7 Currently unused   = 0b0000_0000_1000_0000;
    // bit 8 Currently unused   = 0b0000_0001_0000_0000;
    // bit 9 Currently unused   = 0b0000_0010_0000_0000;
    // bit 10 Currently unused  = 0b0000_0100_0000_0000;

    /// Language encoding flag (EFS).
    /// If this bit is set, the filename and comment fields for this file MUST be encoded using UTF-8.
    LanguageEncoding            = 0b0000_1000_0000_0000,
    /// Reserved by PKWARE for enhanced compression.
    ReservedEnhancedCompression = 0b0001_0000_0000_0000,
    /// Set when encrypting the Central Directory to indicate selected data values in the Local Header are masked to hide their actual values.
    Masked                      = 0b0010_0000_0000_0000,
    /// Reserved by PKWARE for alternate streams.
    ReservedAlternateStream     = 0b0100_0000_0000_0000,
    /// Reserved by PKWARE.
    Reserved                    = 0b1000_0000_0000_0000,
}

impl ZipFlags {
    pub(crate) fn matching(flags: u16, matching_flag: Self) -> bool {
        flags & u16::from(matching_flag) != 0
    }

    pub(crate) const fn as_u16(self) -> u16 {
        self as u16
    }
}

impl From<ZipFlags> for u16 {
    fn from(value: ZipFlags) -> u16 {
        value.as_u16()
    }
}
