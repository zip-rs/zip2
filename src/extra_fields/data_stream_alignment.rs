//! Data stream alignment extra field

use core::mem;
use std::io::{Read, Write};

use crate::extra_fields::UsedExtraField;
use crate::result::{ZipResult, invalid};
use crate::unstable::LittleEndianReadExt;

/// Data stream alignment
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataStreamAlignment {
    /// Extra field header ID (0xa11e for standard DataStreamAlignment, 0xd935 for AndroidZipAlignment)
    pub(crate) header_id: u16,
    /// alignment
    pub(crate) alignment: u16,
    /// padding length
    pub(crate) pad_len: u16,
}

impl DataStreamAlignment {
    /// create the alignment field using the full alignment len needed
    #[must_use]
    pub fn new(padding_len: u16, alignment: u16) -> Option<Self> {
        Self::new_with_header_id(
            UsedExtraField::DataStreamAlignment.as_u16(),
            padding_len,
            alignment,
        )
    }

    /// create an Android alignment field (header ID `0xd935`) using the full alignment len needed
    #[must_use]
    pub fn android(padding_len: u16, alignment: u16) -> Option<Self> {
        Self::new_with_header_id(
            UsedExtraField::AndroidZipAlignment.as_u16(),
            padding_len,
            alignment,
        )
    }

    /// create an alignment field with an explicit header ID using the full alignment len needed
    #[must_use]
    pub fn new_with_header_id(header_id: u16, padding_len: u16, alignment: u16) -> Option<Self> {
        if padding_len >= 6 {
            Some(Self {
                header_id,
                alignment,
                pad_len: padding_len.saturating_sub(4),
            })
        } else {
            None
        }
    }

    /// Returns the extra field header ID (`0xa11e` for DataStreamAlignment, `0xd935` for AndroidZipAlignment).
    #[must_use]
    pub const fn header_id(&self) -> u16 {
        self.header_id
    }

    /// Returns the requested alignment multiple.
    #[must_use]
    pub const fn alignment(&self) -> u16 {
        self.alignment
    }

    /// Returns the padding length stored in the extra field (including the 2-byte alignment multiple).
    #[must_use]
    pub const fn pad_len(&self) -> u16 {
        self.pad_len
    }

    pub(crate) fn try_from_reader<R: Read>(
        reader: &mut R,
        header_id: u16,
        len: u16,
    ) -> ZipResult<Self> {
        if len < 2 {
            return Err(invalid!("Alignment extra field too short"));
        }
        let alignment = reader.read_u16_le()?;
        let mut remaining = (len - 2) as usize;
        let mut buf = [0u8; 1024];
        while remaining > 0 {
            let to_read = remaining.min(buf.len());
            reader.read_exact(&mut buf[..to_read])?;
            remaining -= to_read;
        }
        Ok(Self {
            header_id,
            alignment,
            pad_len: len,
        })
    }

    pub(crate) fn full_size(&self, is_local_header: bool) -> usize {
        if is_local_header {
            mem::size_of::<u16>() + mem::size_of::<u16>() + self.pad_len as usize
        } else {
            // no padding in central header
            0
        }
    }

    pub(crate) fn write<W: Write>(
        &self,
        writer: &mut W,
        is_local_header: bool,
    ) -> std::io::Result<()> {
        if !is_local_header {
            // no padding in central header
            return Ok(());
        }
        writer.write_all(&self.header_id.to_le_bytes())?;
        writer.write_all(&self.pad_len.to_le_bytes())?;
        writer.write_all(&self.alignment.to_le_bytes())?;
        let pad_len = self.pad_len.saturating_sub(2);
        let zeros = [0u8; 1024];
        let mut remaining = pad_len as usize;
        while remaining > 0 {
            let n = remaining.min(zeros.len());
            writer.write_all(&zeros[..n])?;
            remaining -= n;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_data_stream_alignment_constructors() {
        assert!(DataStreamAlignment::new(5, 16).is_none());
        let dsa = DataStreamAlignment::new(6, 16).unwrap();
        assert_eq!(dsa.header_id(), 0xa11e);
        assert_eq!(dsa.alignment(), 16);
        assert_eq!(dsa.pad_len(), 2);
        assert_eq!(dsa.full_size(true), 6);
        assert_eq!(dsa.full_size(false), 0);

        let android_dsa = DataStreamAlignment::android(10, 4).unwrap();
        assert_eq!(android_dsa.header_id(), 0xd935);
        assert_eq!(android_dsa.alignment(), 4);
        assert_eq!(android_dsa.pad_len(), 6);
        assert_eq!(android_dsa.full_size(true), 10);
    }

    #[test]
    fn test_data_stream_alignment_roundtrip() {
        let original = DataStreamAlignment::android(8, 4).unwrap();
        let mut written = Vec::new();
        original.write(&mut written, true).unwrap();
        assert_eq!(written.len(), 8);
        assert_eq!(&written[0..2], &(0xd935_u16).to_le_bytes());
        assert_eq!(&written[2..4], &(4_u16).to_le_bytes()); // pad_len = 8 - 4 = 4
        assert_eq!(&written[4..6], &(4_u16).to_le_bytes()); // alignment = 4
        assert_eq!(&written[6..8], &[0, 0]); // 2 zero padding bytes

        // Read payload back (header_id and len are read by ExtraField::parse)
        let mut cursor = Cursor::new(&written[4..]);
        let parsed = DataStreamAlignment::try_from_reader(&mut cursor, 0xd935, 4).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn test_data_stream_alignment_invalid_short() {
        let mut cursor = Cursor::new([0u8; 1]);
        assert!(DataStreamAlignment::try_from_reader(&mut cursor, 0xd935, 1).is_err());
    }
}
