//! Extended Timestamp extra field
//! Defined in <https://libzip.org/specifications/extrafld.txt>

use crate::extra_fields::UsedExtraField;
use crate::result::invalid;
use crate::result::{ZipError, ZipResult};
use crate::unstable::LittleEndianReadExt;
use core::mem;
use std::io::{Read, Write};

/// `ExtendedTimestamp` Flags
#[rustfmt::skip]
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum ExtendedTimestampFlags {
    /// modification time is present
    Modified = 0b0000_0001_u8,
    /// access time is present
    Accessed = 0b0000_0010_u8,
    /// creation time is present
    Created  = 0b0000_0100_u8,
    // others bytes reversed
}

impl ExtendedTimestampFlags {
    #[inline(always)]
    fn matching(flags: u8, matching_flag: Self) -> bool {
        flags & u8::from(matching_flag) != 0
    }
}

impl From<ExtendedTimestampFlags> for u8 {
    fn from(extended_timestamp: ExtendedTimestampFlags) -> u8 {
        extended_timestamp as u8
    }
}

/// Extended timestamp, as described in <https://libzip.org/specifications/extrafld.txt>
#[derive(Debug, Clone, PartialEq)]
pub struct ExtendedTimestamp {
    modified: Option<u32>,
    accessed: Option<u32>,
    created: Option<u32>,
}

impl ExtendedTimestamp {
    const MAX_LENGTH: u16 = (mem::size_of::<u8>()
        + mem::size_of::<u32>()
        + mem::size_of::<u32>()
        + mem::size_of::<u32>()) as u16;
    /// creates an extended timestamp struct by reading the required bytes from the reader.
    ///
    /// This method assumes that the length has already been read, therefore
    /// it must be passed as an argument
    ///
    /// # Errors
    ///
    /// Fails if the len is empty or a read fails
    ///
    pub fn try_from_reader<R: Read>(
        reader: &mut R,
        len: u16,
        is_local_header: bool,
    ) -> ZipResult<Self> {
        if len == 0 {
            return Err(invalid!("Extended timestamp field is empty"));
        }
        let mut flags = [0u8];
        reader.read_exact(&mut flags)?;
        let bytes_to_read = (len as usize)
            .checked_sub(flags.len())
            .ok_or_else(|| invalid!("Extended timestamp field too short for flags"))?;

        if is_local_header {
            let flags = flags[0];
            return Self::parse_local_header(reader, len, flags, bytes_to_read);
        }
        // the flags field refer to the local header
        Self::parse_central_header(reader, bytes_to_read)
    }

    /// parse the local header
    fn parse_local_header<R: Read>(
        reader: &mut R,
        len: u16,
        flags: u8,
        mut bytes_to_read: usize,
    ) -> ZipResult<Self> {
        // the `flags` field refers to the local headers and might not correspond
        // to the len field. If the length field is 1+4, we assume that only
        // the modification time has been set

        // > Those times that are present will appear in the order indicated, but
        // > any combination of times may be omitted.  (Creation time may be
        // > present without access time, for example.)  TSize should equal
        // > (1 + 4*(number of set bits in Flags)), as the block is currently
        // > defined.
        if len != 5 && u32::from(len) != 1 + 4 * flags.count_ones() {
            return Err(ZipError::Io(std::io::Error::other(format!(
                "flags and len don't match in extended timestamp field len={len} flags={flags:08b}"
            ))));
        }

        let modified =
            if (ExtendedTimestampFlags::matching(flags, ExtendedTimestampFlags::Modified)
                && bytes_to_read >= mem::size_of::<u32>())
                || len == Self::MAX_LENGTH
            {
                bytes_to_read = bytes_to_read.checked_sub(mem::size_of::<u32>()).ok_or_else(||
                    invalid!(
                        "Extended timestamp field too short for mod_time len={} flags={flags:08b}",
                        len
                    ),
                )?;
                Some(reader.read_u32_le()?)
            } else {
                None
            };

        let accessed =
            if (ExtendedTimestampFlags::matching(flags, ExtendedTimestampFlags::Accessed)
                && bytes_to_read >= mem::size_of::<u32>())
                || len == Self::MAX_LENGTH
            {
                bytes_to_read = bytes_to_read.checked_sub(mem::size_of::<u32>()).ok_or_else(||
                    invalid!(
                        "Extended timestamp field too short for ac_time len={} flags={flags:08b}",
                        len
                    ),
                )?;
                Some(reader.read_u32_le()?)
            } else {
                None
            };

        let created =
            if (ExtendedTimestampFlags::matching(flags, ExtendedTimestampFlags::Created)
                && bytes_to_read >= mem::size_of::<u32>())
                || len == Self::MAX_LENGTH
            {
                bytes_to_read = bytes_to_read
                .checked_sub(mem::size_of::<u32>())
                .ok_or_else(|| invalid!(
                    "Extended timestamp field too short for cr_time len={} flags={flags:08b}",
                    len
                ))?;
                Some(reader.read_u32_le()?)
            } else {
                None
            };

        if bytes_to_read > 0 {
            // ignore undocumented bytes
            let n = std::io::copy(&mut reader.take(bytes_to_read as u64), &mut std::io::sink())?;

            if n != bytes_to_read as u64 {
                return Err(invalid!(
                    "unexpected EOF in local header of Extended Timestamp extra field"
                ));
            }
        }

        Ok(Self {
            modified,
            accessed,
            created,
        })
    }

    /// parse the central header
    fn parse_central_header<R: Read>(reader: &mut R, mut bytes_to_read: usize) -> ZipResult<Self> {
        let modified = if bytes_to_read >= mem::size_of::<u32>() {
            bytes_to_read -= mem::size_of::<u32>();
            Some(reader.read_u32_le()?)
        } else {
            None
        };
        if bytes_to_read > 0 {
            // ignore undocumented bytes
            let n = std::io::copy(&mut reader.take(bytes_to_read as u64), &mut std::io::sink())?;

            if n != bytes_to_read as u64 {
                return Err(invalid!(
                    "unexpected EOF in central header of Extended Timestamp extra field"
                ));
            }
        }
        Ok(Self {
            modified,
            accessed: None,
            created: None,
        })
    }

    /// returns the last modification timestamp, if defined, as UNIX epoch seconds
    #[must_use]
    pub fn mod_time(&self) -> Option<u32> {
        self.modified
    }

    /// returns the last access timestamp, if defined, as UNIX epoch seconds
    #[must_use]
    pub fn ac_time(&self) -> Option<u32> {
        self.accessed
    }

    /// returns the creation timestamp, if defined, as UNIX epoch seconds
    #[must_use]
    pub fn cr_time(&self) -> Option<u32> {
        self.created
    }

    /// Number of timestamps the serialized field contains.
    ///
    /// The central header only stores the modification time, so the access and
    /// creation times are left out there.
    fn time_count(&self, is_local_header: bool) -> usize {
        let mut count = usize::from(self.modified.is_some());
        if is_local_header {
            count += usize::from(self.accessed.is_some());
            count += usize::from(self.created.is_some());
        }
        count
    }

    /// Size of the serialized extra field, including its 4-byte extra field
    /// header (tag and data size).
    pub(crate) fn full_size(&self, is_local_header: bool) -> usize {
        mem::size_of::<u16>() * 2
            + mem::size_of::<u8>()
            + mem::size_of::<u32>() * self.time_count(is_local_header)
    }

    /// Serializes the extra field, including its tag and data size.
    ///
    /// This keeps the timestamps of a copied entry, whose local header would
    /// otherwise only carry the rounded, local-time DOS date-time.
    pub(crate) fn write<W: Write>(&self, write: &mut W, is_local_header: bool) -> ZipResult<()> {
        // Only the stored timestamps are advertised: the central header keeps
        // the modification time only, and flags that don't match the data would
        // make the field unreadable.
        let mut flags = 0u8;
        if self.modified.is_some() {
            flags |= u8::from(ExtendedTimestampFlags::Modified);
        }
        if is_local_header {
            if self.accessed.is_some() {
                flags |= u8::from(ExtendedTimestampFlags::Accessed);
            }
            if self.created.is_some() {
                flags |= u8::from(ExtendedTimestampFlags::Created);
            }
        }

        write.write_all(&UsedExtraField::ExtendedTimestamp.to_le_bytes())?;
        let data_size = (self.full_size(is_local_header) - mem::size_of::<u16>() * 2) as u16;
        write.write_all(&data_size.to_le_bytes())?;
        write.write_all(&[flags])?;

        if let Some(modified) = self.modified {
            write.write_all(&modified.to_le_bytes())?;
        }
        if is_local_header {
            if let Some(accessed) = self.accessed {
                write.write_all(&accessed.to_le_bytes())?;
            }
            if let Some(created) = self.created {
                write.write_all(&created.to_le_bytes())?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ExtendedTimestamp;

    use std::io::Cursor;

    /// Ensure we don't panic or read garbage data if the field body is empty
    #[test]
    pub fn test_bad_extended_timestamp() {
        use crate::ZipArchive;

        assert!(
            ZipArchive::new(Cursor::new(include_bytes!(
                "../../tests/data/extended_timestamp_bad.zip"
            )))
            .is_err()
        );
    }

    /// Ensure that a truncated extended timestamp (len too short for flags)
    /// returns an error instead of panicking from a subtraction overflow.
    #[test]
    fn test_extended_timestamp_overflow() {
        let tests_args = [
            (vec![0b0000_0001_u8, 0x00, 0x00, 0x00], 0), // len is 0
            (vec![0b0000_0001_u8, 0x00, 0x00, 0x00], 2), // len is 2 (not enough)
            (vec![0b0000_0010_u8, 0x00, 0x00, 0x00], 3), // len is 3 (not enough)
            (vec![0b0000_0100_u8, 0x00, 0x00, 0x00], 4), // len is 4 (not enough)
            // len is 8 (not enough)
            (
                vec![
                    0b0000_0011_u8,
                    0x00,
                    0x00,
                    0x00,
                    0x00,
                    0x00,
                    0x00,
                    0x00,
                    0x00,
                ],
                8,
            ),
            // len is too long and missing 4 bytes
            (vec![0b0000_0011_u8, 0x00, 0x00, 0x00, 0x00], 8),
            (vec![0b0000_0000_u8], 5), // too long len
        ];
        for (data, len) in tests_args {
            let mut cursor = Cursor::new(data);
            let result = ExtendedTimestamp::try_from_reader(&mut cursor, len, true);
            assert!(result.is_err());
        }
    }

    #[test]
    fn check_extended_timestamp_value() {
        let mut cursor = Cursor::new(&[0b0000_0001_u8, 0x00, 0x00, 0x00, 0x01]);
        let result = ExtendedTimestamp::try_from_reader(&mut cursor, 5, true).unwrap();
        assert_eq!(result.mod_time(), Some(1 << 24));
        assert_eq!(result.ac_time(), None);
        assert_eq!(result.cr_time(), None);

        let mut cursor = Cursor::new(&[0b0000_0010_u8, 0x00, 0x00, 0x00, 0x02]);
        let result = ExtendedTimestamp::try_from_reader(&mut cursor, 5, true).unwrap();
        assert_eq!(result.mod_time(), None);
        assert_eq!(result.ac_time(), Some(2 << 24));
        assert_eq!(result.cr_time(), None);

        let mut cursor = Cursor::new(&[0b0000_0100_u8, 0x00, 0x00, 0x00, 0x03]);
        let result = ExtendedTimestamp::try_from_reader(&mut cursor, 5, true).unwrap();
        assert_eq!(result.mod_time(), None);
        assert_eq!(result.ac_time(), None);
        assert_eq!(result.cr_time(), Some(3 << 24));

        let mut cursor = Cursor::new(&[
            0b0000_0011_u8,
            0x00,
            0x00,
            0x00,
            0x01,
            0x00,
            0x00,
            0x00,
            0x02,
        ]);
        let result = ExtendedTimestamp::try_from_reader(&mut cursor, 9, true).unwrap();
        assert_eq!(result.mod_time(), Some(1 << 24));
        assert_eq!(result.ac_time(), Some(2 << 24));
        assert_eq!(result.cr_time(), None);

        let mut cursor = Cursor::new(&[
            0b0000_0111_u8,
            0x00,
            0x00,
            0x00,
            0x01,
            0x00,
            0x00,
            0x00,
            0x02,
            0x00,
            0x00,
            0x00,
            0x03,
        ]);
        let result = ExtendedTimestamp::try_from_reader(&mut cursor, 13, true).unwrap();
        assert_eq!(result.mod_time(), Some(1 << 24));
        assert_eq!(result.ac_time(), Some(2 << 24));
        assert_eq!(result.cr_time(), Some(3 << 24));
    }

    #[test]
    fn test_extended_timestamp() {
        // in the central header
        let mut cursor = Cursor::new(&[0b0000_0111_u8, 0x00, 0x00, 0x00, 0x01]);
        let result = ExtendedTimestamp::try_from_reader(&mut cursor, 5, true).unwrap();
        assert_eq!(result.mod_time(), Some(1 << 24));
        assert_eq!(result.ac_time(), None);
        assert_eq!(result.cr_time(), None);

        // in the local header
        let mut cursor = Cursor::new(&[
            0b0000_0111_u8,
            0x00,
            0x00,
            0x00,
            0x01,
            0x00,
            0x00,
            0x00,
            0x02,
            0x00,
            0x00,
            0x00,
            0x03,
        ]);
        let result = ExtendedTimestamp::try_from_reader(&mut cursor, 13, true).unwrap();
        assert_eq!(result.mod_time(), Some(1 << 24));
        assert_eq!(result.ac_time(), Some(2 << 24));
        assert_eq!(result.cr_time(), Some(3 << 24));
    }

    #[test]
    fn test_extended_timestamp_in_central_header() {
        let mut cursor = Cursor::new(&[0b0000_0111_u8]);
        let result = ExtendedTimestamp::try_from_reader(&mut cursor, 1, false).unwrap();
        assert_eq!(result.mod_time(), None);
        assert_eq!(result.ac_time(), None);
        assert_eq!(result.cr_time(), None);

        let mut cursor = Cursor::new(&[0b0000_0100_u8, 0x00, 0xF1, 0x53, 0x65]);
        let result = ExtendedTimestamp::try_from_reader(&mut cursor, 5, false).unwrap();
        assert_eq!(result.mod_time(), Some(1700000000));
        assert_eq!(result.ac_time(), None);
        assert_eq!(result.cr_time(), None);

        // a writer that copies the local field into the central directory
        let data = [
            0b0000_0011_u8,
            0x00,
            0xF1,
            0x53,
            0x65,
            0x01,
            0xF1,
            0x53,
            0x65,
        ];
        let mut cursor = Cursor::new(&data);
        let result = ExtendedTimestamp::try_from_reader(&mut cursor, 9, false).unwrap();
        assert_eq!(result.mod_time(), Some(1700000000));
        assert_eq!(cursor.position(), 9);
    }

    /// A copied entry must be able to serialize the field again, with the same
    /// timestamps and flags that the source advertised.
    #[test]
    fn test_write_local_header() {
        let field = [
            0b0000_0111_u8,
            0x01,
            0xF1,
            0x53,
            0x65,
            0x02,
            0xF1,
            0x53,
            0x65,
            0x03,
            0xF1,
            0x53,
            0x65,
        ];
        let mut cursor = Cursor::new(&field);
        let timestamp = ExtendedTimestamp::try_from_reader(&mut cursor, 13, true).unwrap();
        let mut serialized = Vec::new();
        timestamp.write(&mut serialized, true).unwrap();
        assert_eq!(serialized.len(), 4 + field.len());
        assert_eq!(&serialized[4..], &field[..]);
    }

    /// The central header stores the modification time only, so its flags must
    /// not advertise the access and creation times.
    #[test]
    fn test_write_central_header() {
        let field = [
            0b0000_0111_u8,
            0x01,
            0xF1,
            0x53,
            0x65,
            0x02,
            0xF1,
            0x53,
            0x65,
            0x03,
            0xF1,
            0x53,
            0x65,
        ];
        let mut cursor = Cursor::new(&field);
        let timestamp = ExtendedTimestamp::try_from_reader(&mut cursor, 13, true).unwrap();
        let mut serialized = Vec::new();
        timestamp.write(&mut serialized, false).unwrap();
        assert_eq!(serialized.len(), 4 + 5);
        assert_eq!(&serialized[4..], &[0b0000_0001, 0x01, 0xF1, 0x53, 0x65]);
    }
}
