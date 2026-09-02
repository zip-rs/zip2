//! "Magic" header values used in the zip spec to locate metadata records.

/// These values currently always take up a fixed four bytes, so we can parse and wrap them in this
/// struct to enforce some small amount of type safety.
#[derive(Copy, Clone, Debug, PartialOrd, Ord, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Magic(u32);

impl Magic {
    /// Local file header signature
    pub const LOCAL_FILE_HEADER_SIGNATURE: Self = Self::literal(0x0403_4b50);
    /// Central directory header signature
    pub const CENTRAL_DIRECTORY_HEADER_SIGNATURE: Self = Self::literal(0x0201_4b50);
    /// Central directory end signature
    pub const CENTRAL_DIRECTORY_END_SIGNATURE: Self = Self::literal(0x0605_4b50);
    /// Zip64 central directory signature
    pub const ZIP64_CENTRAL_DIRECTORY_END_SIGNATURE: Self = Self::literal(0x0606_4b50);
    /// Zip64 central directory end locator signature
    pub const ZIP64_CENTRAL_DIRECTORY_END_LOCATOR_SIGNATURE: Self = Self::literal(0x0706_4b50);
    /// Data descriptor signature
    pub const DATA_DESCRIPTOR_SIGNATURE: Self = Self::literal(0x0807_4b50);

    /// Create new literal
    #[must_use]
    pub const fn literal(x: u32) -> Self {
        Self(x)
    }

    /// Create new from bytes
    #[inline(always)]
    #[must_use]
    pub const fn from_le_bytes(bytes: [u8; 4]) -> Self {
        Self(u32::from_le_bytes(bytes))
    }

    /// Get as bytes
    #[inline(always)]
    #[must_use]
    pub const fn to_le_bytes(self) -> [u8; 4] {
        self.0.to_le_bytes()
    }

    /// From little endian
    #[allow(clippy::wrong_self_convention)]
    #[inline(always)]
    #[must_use]
    pub fn from_le(self) -> Self {
        Self(u32::from_le(self.0))
    }

    /// To little endian
    #[allow(clippy::wrong_self_convention)]
    #[inline(always)]
    #[must_use]
    pub fn to_le(self) -> Self {
        Self(u32::to_le(self.0))
    }
}
