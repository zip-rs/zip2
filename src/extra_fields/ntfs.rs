use std::io::{Read, Write};
use core::mem;

use crate::{
    extra_fields::UsedExtraField,
    result::{ZipError, ZipResult},
    unstable::LittleEndianReadExt,
};

/// The NTFS extra field as described in [PKWARE's APPNOTE.TXT v6.3.9].
///
/// This field stores [Windows file times], which are 64-bit unsigned integer
/// values that represents the number of 100-nanosecond intervals that have
/// elapsed since "1601-01-01 00:00:00 UTC".
///
/// [PKWARE's APPNOTE.TXT v6.3.9]: https://pkware.cachefly.net/webdocs/casestudies/APPNOTE.TXT
/// [Windows file times]: https://docs.microsoft.com/en-us/windows/win32/sysinfo/file-times
#[derive(Clone, Debug, PartialEq)]
pub struct Ntfs {
    mtime: u64,
    atime: u64,
    ctime: u64,
}

impl Ntfs {
    /// Creates a NTFS extra field struct by reading the required bytes from the
    /// reader.
    ///
    /// This method assumes that the length has already been read, therefore it
    /// must be passed as an argument.
    ///
    /// # Errors
    ///
    /// Fails if the NTFS extra field doesn't have the correct length or a read fails
    ///
    pub fn try_from_reader<R>(reader: &mut R, len: u16) -> ZipResult<Self>
    where
        R: Read,
    {
        if len != 32 {
            return Err(ZipError::UnsupportedArchive(
                "NTFS extra field has an unsupported length",
            ));
        }

        // Read reserved for future use.
        let _ = reader.read_u32_le()?;

        let tag = reader.read_u16_le()?;
        if tag != 0x0001 {
            return Err(ZipError::UnsupportedArchive(
                "NTFS extra field has an unsupported attribute tag",
            ));
        }
        let size = reader.read_u16_le()?;
        if size != 24 {
            return Err(ZipError::UnsupportedArchive(
                "NTFS extra field has an unsupported attribute size",
            ));
        }

        let mtime = reader.read_u64_le()?;
        let atime = reader.read_u64_le()?;
        let ctime = reader.read_u64_le()?;
        Ok(Self {
            mtime,
            atime,
            ctime,
        })
    }

    /// Returns the file last modification time as a file time.
    #[must_use]
    pub fn mtime(&self) -> u64 {
        self.mtime
    }

    /// Returns the file last modification time as a file time.
    #[cfg(feature = "nt-time")]
    #[must_use]
    pub fn modified_file_time(&self) -> nt_time::FileTime {
        nt_time::FileTime::new(self.mtime)
    }

    /// Returns the file last access time as a file time.
    #[must_use]
    pub fn atime(&self) -> u64 {
        self.atime
    }

    /// Returns the file last access time as a file time.
    #[cfg(feature = "nt-time")]
    #[must_use]
    pub fn accessed_file_time(&self) -> nt_time::FileTime {
        nt_time::FileTime::new(self.atime)
    }

    /// Returns the file creation time as a file time.
    #[must_use]
    pub fn ctime(&self) -> u64 {
        self.ctime
    }

    /// Returns the file creation time as a file time.
    #[cfg(feature = "nt-time")]
    #[must_use]
    pub fn created_file_time(&self) -> nt_time::FileTime {
        nt_time::FileTime::new(self.ctime)
    }

    /// Creates a new `Ntfs` extra field with the given 100-nanosecond Windows file times
    /// (intervals since 1601-01-01 00:00:00 UTC).
    #[must_use]
    pub const fn new(mtime: u64, atime: u64, ctime: u64) -> Self {
        Self {
            mtime,
            atime,
            ctime,
        }
    }

    #[cfg(feature = "nt-time")]
    /// Creates a new `Ntfs` extra field from [`nt_time::FileTime`] timestamps.
    #[must_use]
    pub fn from_file_times(
        mtime: nt_time::FileTime,
        atime: nt_time::FileTime,
        ctime: nt_time::FileTime,
    ) -> Self {
        Self::new(mtime.to_raw(), atime.to_raw(), ctime.to_raw())
    }

    /// Size of the serialized extra field, including its 4-byte extra field
    /// header (tag and data size).
    pub(crate) fn full_size(&self) -> usize {
        mem::size_of::<u16>() * 2 // extra field tag and data size
            + mem::size_of::<u32>() // reserved for future use
            + mem::size_of::<u16>() * 2 // attribute tag and attribute size
            + 3 * mem::size_of::<u64>() // mtime, atime and ctime
    }

    /// Serializes the extra field, including its tag and data size.
    ///
    /// This keeps the file times of a copied entry, which a copy that only
    /// carries the DOS date-time in the local header would round and shift.
    pub(crate) fn write<W: Write>(&self, write: &mut W) -> ZipResult<()> {
        write.write_all(&UsedExtraField::Ntfs.to_le_bytes())?;
        let data_size = (self.full_size() - mem::size_of::<u16>() * 2) as u16;
        write.write_all(&data_size.to_le_bytes())?;
        write.write_all(&0u32.to_le_bytes())?; // reserved for future use
        write.write_all(&0x0001u16.to_le_bytes())?; // attribute tag: file times
        write.write_all(&24u16.to_le_bytes())?; // attribute size
        write.write_all(&self.mtime.to_le_bytes())?;
        write.write_all(&self.atime.to_le_bytes())?;
        write.write_all(&self.ctime.to_le_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Ntfs;
    use std::io::Cursor;

    #[test]
    fn test_ntfs_constructors_and_write() {
        let ntfs = Ntfs::new(100, 200, 300);
        assert_eq!(ntfs.mtime(), 100);
        assert_eq!(ntfs.atime(), 200);
        assert_eq!(ntfs.ctime(), 300);
        assert_eq!(ntfs.full_size(), 36);

        let mut buf = Vec::new();
        ntfs.write(&mut buf).unwrap();
        assert_eq!(buf.len(), 36);

        // Header ID 0x000A, Data Size 32
        assert_eq!(&buf[0..2], &0x000Au16.to_le_bytes());
        assert_eq!(&buf[2..4], &32u16.to_le_bytes());

        // Parse it back
        let mut cursor = Cursor::new(&buf[4..]);
        let parsed = Ntfs::try_from_reader(&mut cursor, 32).unwrap();
        assert_eq!(parsed, ntfs);
    }
}
