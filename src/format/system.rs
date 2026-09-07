//! Zip System

/// System inside `version made by` (upper byte)
/// Reference: 4.4.2.2
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(clippy::upper_case_acronyms)]
#[repr(u8)]
#[non_exhaustive]
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

#[cfg(test)]
mod tests {
    #[test]
    fn system() {
        use super::System;
        assert_eq!(u8::from(System::Dos), 0u8);
        assert_eq!(System::Dos as u8, 0u8);
        assert_eq!(System::Unix as u8, 3u8);
        assert_eq!(u8::from(System::Unix), 3u8);
        assert_eq!(System::from(0), System::Dos);
        assert_eq!(System::from(3), System::Unix);
        assert_eq!(u8::from(System::Unknown), 255u8);
        assert_eq!(System::Unknown as u8, 255u8);
    }
}
