//! Data stream aligment extra field

use core::mem;
use std::io::Write;

use crate::extra_fields::UsedExtraField;

/// Data stream alignement
#[derive(Debug, Clone)]
pub struct DataStreamAlignment {
    /// padding lenght
    pad_len: u16,
}

impl DataStreamAlignment {
    /// create the alignment field using the full alignment len needed
    #[must_use]
    pub fn new(alignment_len: u16) -> Option<Self> {
        if alignment_len >= 6 {
            Some(Self {
                pad_len: alignment_len.saturating_sub(4),
            })
        } else {
            None
        }
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
        let magic = UsedExtraField::DataStreamAlignment.as_u16();
        writer.write_all(&magic.to_le_bytes())?;
        writer.write_all(&self.pad_len.to_le_bytes())?;
        let pad_len = self.pad_len.saturating_sub(2);
        writer.write_all(&pad_len.to_le_bytes())?;
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
