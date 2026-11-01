//! Compression

/// Compression
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[repr(u16)]
#[non_exhaustive]
pub enum Compression {
    /// Store the file as is
    #[default]
    Stored = 0,
    /// Method 1 Shrink
    Shrink = 1,
    /// Reduce with compression factor 1
    ReduceFactor1 = 2,
    /// Reduce with compression factor 2
    ReduceFactor2 = 3,
    /// Reduce with compression factor 3
    ReduceFactor3 = 4,
    /// Reduce with compression factor 4
    ReduceFactor4 = 5,
    /// Implode/explode
    Implode = 6,
    /// Reserved for Tokenizing compression algorithm
    TokenizingCompression = 7,
    /// Compress the file using Deflate
    Deflated = 8,
    /// Compress the file using Deflate64.
    Deflate64 = 9,
    /// PKWARE Data Compression Library Imploding (old IBM TERSE)
    PkwareDataCompressionImploding = 10,
    /// Reserved by PKWARE
    Reserved11 = 11,
    /// Compress the file using BZIP2
    Bzip2 = 12,
    /// Reserved by PKWARE
    Reserved13 = 13,
    /// Compress the file using LZMA
    Lzma = 14,
    /// Reserved by PKWARE
    Reserved15 = 15,
    /// IBM z/OS CMPSC Compression
    CmpscCompression = 16,
    /// Reserved by PKWARE
    Reserved17 = 17,
    /// File is compressed using IBM TERSE (new)
    IbmTerseNew = 18,
    /// IBM LZ77 z Architecture
    IbmLz77 = 19,
    /// deprecated ZSTD
    DeprecatedZstd = 20,
    /// Compress the file using `ZStandard`
    Zstd = 93,
    /// MP3 Compression
    Mp3 = 94,
    /// Compress the file using XZ
    Xz = 95,
    /// JPEG variant
    Jpeg = 96,
    /// WavPack compressed data
    WavPack = 97,
    /// Compress the file using `PPMd`
    Ppmd = 98,
    /// Encrypted using AES.
    ///
    /// The actual compression method has to be taken from the AES extra data field
    /// or from `ZipFileData`.
    Aes = 99,
    /// Unsupported compression method
    Unknown(u16),
}

impl From<u16> for Compression {
    fn from(value: u16) -> Self {
        match value {
            0 => Self::Stored,
            1 => Self::Shrink,
            2 => Self::ReduceFactor1,
            3 => Self::ReduceFactor2,
            4 => Self::ReduceFactor3,
            5 => Self::ReduceFactor4,
            6 => Self::Implode,
            7 => Self::TokenizingCompression,
            8 => Self::Deflated,
            9 => Self::Deflate64,
            10 => Self::PkwareDataCompressionImploding,
            11 => Self::Reserved11,
            12 => Self::Bzip2,
            13 => Self::Reserved13,
            14 => Self::Lzma,
            15 => Self::Reserved15,
            16 => Self::CmpscCompression,
            17 => Self::Reserved17,
            18 => Self::IbmTerseNew,
            19 => Self::IbmLz77,
            20 => Self::DeprecatedZstd,
            93 => Self::Zstd,
            94 => Self::Mp3,
            95 => Self::Xz,
            96 => Self::Jpeg,
            97 => Self::WavPack,
            98 => Self::Ppmd,
            99 => Self::Aes,
            n => Self::Unknown(n),
        }
    }
}

impl From<Compression> for u16 {
    fn from(value: Compression) -> Self {
        match value {
            Compression::Stored => 0,
            Compression::Shrink => 1,
            Compression::ReduceFactor1 => 2,
            Compression::ReduceFactor2 => 3,
            Compression::ReduceFactor3 => 4,
            Compression::ReduceFactor4 => 5,
            Compression::Implode => 6,
            Compression::TokenizingCompression => 7,
            Compression::Deflated => 8,
            Compression::Deflate64 => 9,
            Compression::PkwareDataCompressionImploding => 10,
            Compression::Reserved11 => 11,
            Compression::Bzip2 => 12,
            Compression::Reserved13 => 13,
            Compression::Lzma => 14,
            Compression::Reserved15 => 15,
            Compression::CmpscCompression => 16,
            Compression::Reserved17 => 17,
            Compression::IbmTerseNew => 18,
            Compression::IbmLz77 => 19,
            Compression::DeprecatedZstd => 20,
            Compression::Zstd => 93,
            Compression::Mp3 => 94,
            Compression::Xz => 95,
            Compression::Jpeg => 96,
            Compression::WavPack => 97,
            Compression::Ppmd => 98,
            Compression::Aes => 99,
            Compression::Unknown(value) => value,
        }
    }
}

impl Compression {
    /// as u16
    #[must_use]
    pub fn as_u16(self) -> u16 {
        u16::from(self)
    }
}
