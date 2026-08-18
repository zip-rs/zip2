//! Read Config

/// Configuration for reading ZIP archives.
#[repr(transparent)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Config {
    /// An offset into the reader to use to find the start of the archive.
    pub archive_offset: ArchiveOffset,
}

/// The offset of the start of the archive from the beginning of the reader.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveOffset {
    /// Detect the archive offset automatically by searching for the central directory's actual
    /// location and the location specified by the End of Central Directory (EOCD) record.
    #[default]
    Detect,
    /// Specify a fixed archive offset.
    Known(u64),
}
