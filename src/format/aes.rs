//! AES related specifications

use core::fmt::Display;

/// The encryption specification used to encrypt a file with AES.
///
/// According to the [specification](https://www.winzip.com/win/en/aes_info.html#winzip11) AE-2
/// does not make use of the CRC check.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum AesVendorVersion {
    Ae1 = 0x0001,
    Ae2 = 0x0002,
}

impl AesVendorVersion {
    /// As u16
    #[must_use]
    pub const fn as_u16(self) -> u16 {
        self as u16
    }

    /// Returns `true` if the data is encrypted using AE2.
    #[cfg(feature = "aes-crypto")]
    pub const fn is_ae2_encrypted(&self) -> bool {
        matches!(self, AesVendorVersion::Ae2)
    }

    /// `false` since the feature `aes-crypto` is not enabled
    #[cfg(not(feature = "aes-crypto"))]
    pub const fn is_ae2_encrypted(&self) -> bool {
        false
    }
}

impl TryFrom<u16> for AesVendorVersion {
    type Error = &'static str;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        let aes_vendor_version = match value {
            0x0001 => AesVendorVersion::Ae1,
            0x0002 => AesVendorVersion::Ae2,
            _ => return Err("Invalid AES vendor version"),
        };
        Ok(aes_vendor_version)
    }
}

impl From<AesVendorVersion> for u16 {
    fn from(value: AesVendorVersion) -> Self {
        value.as_u16()
    }
}

/// AES variant used.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "_arbitrary", derive(arbitrary::Arbitrary))]
#[repr(u8)]
pub enum AesMode {
    /// 128-bit AES encryption.
    Aes128 = 0x01,
    /// 192-bit AES encryption.
    Aes192 = 0x02,
    /// 256-bit AES encryption.
    Aes256 = 0x03,
}

impl AesMode {
    /// As u8
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

impl Display for AesMode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Aes128 => write!(f, "AES-128"),
            Self::Aes192 => write!(f, "AES-192"),
            Self::Aes256 => write!(f, "AES-256"),
        }
    }
}

impl TryFrom<u8> for AesMode {
    type Error = &'static str;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        let mode = match value {
            0x01 => AesMode::Aes128,
            0x02 => AesMode::Aes192,
            0x03 => AesMode::Aes256,
            _ => return Err("Invalid AES encryption strength"),
        };
        Ok(mode)
    }
}

#[cfg(feature = "aes-crypto")]
impl AesMode {
    /// Length of the salt for the given AES mode.
    #[must_use]
    pub const fn salt_length(&self) -> usize {
        self.key_length() / 2
    }

    /// Length of the key for the given AES mode.
    #[must_use]
    pub const fn key_length(&self) -> usize {
        match self {
            Self::Aes128 => 16,
            Self::Aes192 => 24,
            Self::Aes256 => 32,
        }
    }
}
