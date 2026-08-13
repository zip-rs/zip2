//! Zip format

pub(crate) mod aes;
pub(crate) mod flags;
pub(crate) mod magic;

pub(crate) mod ffi {
    /// Regular
    pub const S_IFREG: u32 = 0b1000_0000_0000_0000; // 0o0_100_000
    /// Directory
    pub const S_IFDIR: u32 = 0b0100_0000_0000_0000; // 0o0_040_000
    /// Symbolic link
    pub const S_IFLNK: u32 = 0b1010_0000_0000_0000; // 0o0_120_000
}
