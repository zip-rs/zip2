//! Flags of zip

/// Zip flags
/// Stored as Little endian
#[rustfmt::skip]
#[repr(u16)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ZipFlags {
    /// If set, indicates that the file is encrypted.
    Encrypted                   = 0b0000_0000_0000_0001,
    /// Compression setting
    #[allow(unused)]
    CompressionSetting          = 0b0000_0000_0000_0010,
    /// Compression setting 2
    #[allow(unused)]
    CompressionSetting2         = 0b0000_0000_0000_0100,
    /// If this bit is set, the fields crc-32, compressed size and uncompressed size are set to zero in the  local header.
    /// The correct values are put in the data descriptor immediately following the compressed data.
    UsingDataDescriptor         = 0b0000_0000_0000_1000,
    /// Reserved for use with method 8, for enhanced deflating.
    #[allow(unused)]
    ReservedEnhancedDeflating   = 0b0000_0000_0001_0000,
    /// If this bit is set, this indicates that the file is compressed patched data.
    #[allow(unused)]
    CompressedPatchedData       = 0b0000_0000_0010_0000,
    /// Strong encryption.
    /// If this bit is set, you MUST set the version needed to extract value to at least 50 and you MUST also set bit 0.
    /// If AES encryption is used, the version needed to extract value MUST be at least 51.
    #[allow(unused)]
    StrongEncryption            = 0b0000_0000_0100_0000,
    // bit 7 Currently unused   = 0b0000_0000_1000_0000;
    // bit 8 Currently unused   = 0b0000_0001_0000_0000;
    // bit 9 Currently unused   = 0b0000_0010_0000_0000;
    // bit 10 Currently unused  = 0b0000_0100_0000_0000;

    /// Language encoding flag (EFS).
    /// If this bit is set, the filename and comment fields for this file MUST be encoded using UTF-8.
    LanguageEncoding            = 0b0000_1000_0000_0000,
    /// Reserved by PKWARE for enhanced compression.
    #[allow(unused)]
    ReservedEnhancedCompression = 0b0001_0000_0000_0000,
    /// Set when encrypting the Central Directory to indicate selected data values in the Local Header are masked to hide their actual values.
    #[allow(unused)]
    Masked                      = 0b0010_0000_0000_0000,
    /// Reserved by PKWARE for alternate streams.
    #[allow(unused)]
    ReservedAlternateStream     = 0b0100_0000_0000_0000,
    /// Reserved by PKWARE.
    #[allow(unused)]
    Reserved                    = 0b1000_0000_0000_0000,
}

impl ZipFlags {
    #[inline]
    pub(crate) fn matching(flags: u16, matching_flag: Self) -> bool {
        flags & u16::from(matching_flag) != 0
    }

    #[inline]
    pub(crate) const fn as_u16(self) -> u16 {
        self as u16
    }
}

impl From<ZipFlags> for u16 {
    fn from(value: ZipFlags) -> u16 {
        value.as_u16()
    }
}

/// Zip flags
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ZipFileFlags(pub(crate) u16);

impl ZipFileFlags {
    /// Check if flags are matching a specific `ZipFlags`
    #[inline]
    #[must_use]
    pub fn matching(&self, matching_flag: ZipFlags) -> bool {
        ZipFlags::matching(self.0, matching_flag)
    }

    /// Check if the encrypted flag is set
    #[inline]
    #[must_use]
    pub fn is_encrypted(&self) -> bool {
        self.matching(ZipFlags::Encrypted)
    }

    /// Check if the data descriptor flag is set
    #[inline]
    #[must_use]
    pub fn is_using_data_descriptor(&self) -> bool {
        self.matching(ZipFlags::UsingDataDescriptor)
    }

    /// Get the inner value
    #[inline]
    #[must_use]
    pub const fn as_u16(self) -> u16 {
        self.0
    }
}

impl From<u16> for ZipFileFlags {
    fn from(value: u16) -> Self {
        ZipFileFlags(value)
    }
}

impl core::ops::BitOrAssign<u16> for ZipFileFlags {
    fn bitor_assign(&mut self, rhs: u16) {
        self.0 |= rhs;
    }
}
