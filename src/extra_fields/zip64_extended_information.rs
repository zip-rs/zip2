//! 4.5.3 -Zip64 Extended Information Extra Field (0x0001)
//!
//! | Value                  | Size    | Description                                  |
//! | ---------------------- | ------- | -------------------------------------------- |
//! | `0x0001`               | 2 bytes | Tag for this "extra" block type              |
//! | Size                   | 2 bytes | Size of this "extra" block                   |
//! | Original Size          | 8 bytes | Original uncompressed file size              |
//! | Compressed Size        | 8 bytes | Size of compressed data                      |
//! | Relative Header Offset | 8 bytes | Offset of local header record                |
//! | Disk Start Number      | 4 bytes | Number of the disk on which this file starts |
//!

use core::mem;
use std::io::{ErrorKind, Read, Write, copy, sink};

use crate::unstable::LittleEndianReadExt;
use crate::{
    ZIP64_BYTES_THR,
    extra_fields::UsedExtraField,
    result::{ZipResult, invalid},
};

/// Zip64 extended information extra field
#[derive(Copy, Clone, Debug)]
pub(crate) struct Zip64ExtendedInformation {
    /// The local header does not contains any `header_start`
    is_local_header: bool,
    uncompressed_size: Option<u64>,
    compressed_size: Option<u64>,
    header_start: Option<u64>,
    // Not used field
    // disk_start: Option<u32>
}

impl Zip64ExtendedInformation {
    pub(crate) const MAGIC: UsedExtraField = UsedExtraField::Zip64ExtendedInfo;

    pub(crate) fn new_local(is_large_file: bool) -> Option<Self> {
        if is_large_file {
            Self::local_header(true, u64::MAX, u64::MAX)
        } else {
            None
        }
    }

    /// This entry in the Local header MUST include BOTH original and compressed file size fields
    /// If the user is using `is_large_file` when the file is not large we force the zip64 extra field
    pub(crate) fn local_header(
        is_large_file: bool,
        uncompressed_size: u64,
        compressed_size: u64,
    ) -> Option<Self> {
        // here - we force if `is_large_file` is `true`
        let should_add_size = is_large_file
            || uncompressed_size >= ZIP64_BYTES_THR
            || compressed_size >= ZIP64_BYTES_THR;
        if !should_add_size {
            return None;
        }
        let uncompressed_size = Some(uncompressed_size);
        let compressed_size = Some(compressed_size);

        // TODO: (unsupported for now)
        // Disk Start Number  4 bytes    Number of the disk on which this file starts

        Some(Self {
            is_local_header: true,
            uncompressed_size,
            compressed_size,
            header_start: None,
        })
    }

    pub(crate) fn central_header(
        is_large_file: bool,
        uncompressed_size: u64,
        compressed_size: u64,
        header_start: u64,
    ) -> Option<Self> {
        let mut size: u16 = 0;
        let uncompressed_size = if is_large_file || uncompressed_size >= ZIP64_BYTES_THR {
            size += mem::size_of::<u64>() as u16;
            Some(uncompressed_size)
        } else {
            None
        };
        let compressed_size = if is_large_file || compressed_size >= ZIP64_BYTES_THR {
            size += mem::size_of::<u64>() as u16;
            Some(compressed_size)
        } else {
            None
        };
        let header_start = if header_start != 0 && header_start >= ZIP64_BYTES_THR {
            size += mem::size_of::<u64>() as u16;
            Some(header_start)
        } else {
            None
        };
        // TODO: (unsupported for now)
        // Disk Start Number  4 bytes    Number of the disk on which this file starts

        if size == 0 {
            // no info added, return early
            return None;
        }

        Some(Self {
            is_local_header: false,
            uncompressed_size,
            compressed_size,
            header_start,
        })
    }

    /// Get the full size of the block
    pub(crate) fn full_size(&self) -> usize {
        self.size() + mem::size_of::<UsedExtraField>() + mem::size_of::<u16>()
    }

    pub(crate) fn size(&self) -> usize {
        let mut size = 0;
        if self.uncompressed_size.is_some() {
            size += mem::size_of::<u64>();
        }
        if self.compressed_size.is_some() {
            size += mem::size_of::<u64>();
        }
        if self.header_start.is_some() {
            size += mem::size_of::<u64>();
        }
        size
    }

    /// Serialize the block
    pub fn write<T: Write>(self, writer: &mut T) -> ZipResult<()> {
        writer.write_all(&Self::MAGIC.to_le_bytes())?;

        if self.is_local_header {
            // the local header does not contains the header start
            if let (Some(uncompressed_size), Some(compressed_size)) =
                (self.uncompressed_size, self.compressed_size)
            {
                let size = (mem::size_of::<u64>() + mem::size_of::<u64>()) as u16;
                writer.write_all(&size.to_le_bytes())?;
                writer.write_all(&u64::to_le_bytes(uncompressed_size))?;
                writer.write_all(&u64::to_le_bytes(compressed_size))?;
            }
            // the else should be unreachable
        } else {
            let size = self.size() as u16;
            writer.write_all(&size.to_le_bytes())?;
            if let Some(uncompressed_size) = self.uncompressed_size {
                writer.write_all(&u64::to_le_bytes(uncompressed_size))?;
            }
            if let Some(compressed_size) = self.compressed_size {
                writer.write_all(&u64::to_le_bytes(compressed_size))?;
            }
            if let Some(header_start) = self.header_start {
                writer.write_all(&u64::to_le_bytes(header_start))?;
            }
        }
        Ok(())
    }

    #[inline]
    pub(crate) fn parse<R: Read>(
        reader: &mut R,
        len: u16,
        uncompressed_size: u64,
        compressed_size: u64,
        header_start: u64,
    ) -> ZipResult<(u64, u64, u64)> {
        let mut consumed_len = 0;
        let new_uncompressed_size = if len >= 24 || uncompressed_size == ZIP64_BYTES_THR {
            let new_uncompressed_size = match reader.read_u64_le() {
                Ok(v) => v,
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                    return Err(invalid!("ZIP64 extra field truncated"));
                }
                Err(e) => return Err(e.into()),
            };
            consumed_len += mem::size_of::<u64>();
            new_uncompressed_size
        } else {
            uncompressed_size
        };

        let new_compressed_size = if len >= 24 || compressed_size == ZIP64_BYTES_THR {
            let new_compressed_size = match reader.read_u64_le() {
                Ok(v) => v,
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                    return Err(invalid!("ZIP64 extra field truncated"));
                }
                Err(e) => return Err(e.into()),
            };
            consumed_len += mem::size_of::<u64>();
            new_compressed_size
        } else {
            compressed_size
        };

        let new_header_start = if len >= 24 || header_start == ZIP64_BYTES_THR {
            let new_header_start = match reader.read_u64_le() {
                Ok(v) => v,
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                    return Err(invalid!("ZIP64 extra field truncated"));
                }
                Err(e) => return Err(e.into()),
            };
            consumed_len += mem::size_of::<u64>();
            new_header_start
        } else {
            header_start
        };

        let Some(leftover_len) = (len as usize).checked_sub(consumed_len) else {
            return Err(invalid!("ZIP64 extra-data field is the wrong length"));
        };
        let mut limited = reader.take(leftover_len as u64);
        if let Err(e) = copy(&mut limited, &mut sink()) {
            if e.kind() == ErrorKind::UnexpectedEof {
                return Err(invalid!("ZIP64 extra field truncated"));
            }
            return Err(e.into());
        }

        Ok((new_uncompressed_size, new_compressed_size, new_header_start))
    }
}
