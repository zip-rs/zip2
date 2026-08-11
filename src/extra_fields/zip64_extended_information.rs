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

/// Zip64 Sizes
/// This entry in the Local header MUST include BOTH original
/// and compressed file size fields.
#[derive(Copy, Clone, Debug)]
pub struct Zip64Sizes {
    pub(crate) uncompressed_size: u64,
    pub(crate) compressed_size: u64,
}

/// Zip64 extended information extra field
#[derive(Copy, Clone, Debug)]
pub struct Zip64ExtendedInformation {
    pub(crate) sizes: Option<Zip64Sizes>,
    pub(crate) header_start: Option<u64>,
    // TODO: (unsupported for now)
    // Disk Start Number  4 bytes    Number of the disk on which this file starts
    // disk_start: Option<u32>
}

impl Zip64ExtendedInformation {
    pub(crate) const MAGIC: UsedExtraField = UsedExtraField::Zip64ExtendedInfo;

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
        Some(Self {
            sizes: Some(Zip64Sizes {
                uncompressed_size,
                compressed_size,
            }),
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
        let sizes = if is_large_file
            || uncompressed_size >= ZIP64_BYTES_THR
            || compressed_size > ZIP64_BYTES_THR
        {
            size += mem::size_of::<u64>() as u16 + mem::size_of::<u64>() as u16;
            Some(Zip64Sizes {
                uncompressed_size,
                compressed_size,
            })
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
            sizes,
            header_start,
        })
    }

    pub(crate) fn full_size(&self, is_local_header: bool) -> usize {
        mem::size_of::<UsedExtraField>() + mem::size_of::<u16>() + self.size(is_local_header)
    }

    pub(crate) fn size(&self, is_local_header: bool) -> usize {
        let mut size = 0;
        if self.sizes.is_some() {
            size += mem::size_of::<u64>() + mem::size_of::<u64>();
        }
        if !is_local_header && self.header_start.is_some() {
            size += mem::size_of::<u64>();
        }
        size
    }

    /// Serialize the block
    pub fn write<T: Write>(&self, writer: &mut T, is_local_header: bool) -> ZipResult<()> {
        writer.write_all(&Self::MAGIC.to_le_bytes())?;
        let size = self.size(is_local_header) as u16;
        writer.write_all(&size.to_le_bytes())?;
        if let Some(Zip64Sizes {
            uncompressed_size,
            compressed_size,
        }) = self.sizes
        {
            writer.write_all(&u64::to_le_bytes(uncompressed_size))?;
            writer.write_all(&u64::to_le_bytes(compressed_size))?;
        }

        // the local header does not contains the header start
        if !is_local_header && let Some(header_start) = self.header_start {
            writer.write_all(&u64::to_le_bytes(header_start))?;
        }
        Ok(())
    }

    #[inline]
    pub(crate) fn parse<R: Read>(
        reader: &mut R,
        len: u16,
        uncompressed_size: u32,
        compressed_size: u32,
        header_start: Option<u32>,
    ) -> ZipResult<(u64, u64, u64)> {
        let mut consumed_len = 0;
        let new_uncompressed_size = if len >= 24 || u64::from(uncompressed_size) == ZIP64_BYTES_THR
        {
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
            uncompressed_size.into()
        };

        let new_compressed_size = if len >= 24 || u64::from(compressed_size) == ZIP64_BYTES_THR {
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
            compressed_size.into()
        };

        let new_header_start = if len >= 24 {
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
            if let Some(header_start) = header_start {
                if u64::from(header_start) == ZIP64_BYTES_THR {
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
                    header_start.into()
                }
            } else {
                0
            }
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
