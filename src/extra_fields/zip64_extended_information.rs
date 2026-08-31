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
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Zip64Sizes {
    pub(crate) uncompressed_size: u64,
    pub(crate) compressed_size: u64,
}

/// Zip64 extended information extra field
#[derive(Copy, Clone, Debug, PartialEq)]
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

    /// Reads one of the block's 8 byte values, reporting a short read as a truncated block.
    fn read_value<R: Read>(reader: &mut R) -> ZipResult<u64> {
        match reader.read_u64_le() {
            Ok(value) => Ok(value),
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                Err(invalid!("ZIP64 extra field truncated"))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Reads the value for one field, and keeps it only if the entry asked for it.
    ///
    /// Whether the value is *read* is decided by the block's length, so that the reader stays in
    /// step with writers that emit more fields than they need to. Whether it is *kept* is decided
    /// by `is_zip64`, which says whether the matching field in the entry held the sentinel.
    fn read_field<R: Read>(
        reader: &mut R,
        len: u16,
        consumed_len: &mut usize,
        is_zip64: bool,
    ) -> ZipResult<Option<u64>> {
        if len < 24 && !is_zip64 {
            return Ok(None);
        }
        let value = Self::read_value(reader)?;
        *consumed_len += mem::size_of::<u64>();
        Ok(is_zip64.then_some(value))
    }

    /// Reads the block, keeping only the values the entry actually asked for.
    ///
    /// Per APPNOTE 4.5.3 a value belongs in this block only when the matching field in the entry
    /// itself holds the 0xFFFFFFFF sentinel, which means "too large to store here, the real value
    /// is in the ZIP64 block". Writers exist that emit a full length block anyway, and at least
    /// one emits a malformed one, so a value that no sentinel asked for is dropped rather than
    /// read over the entry's own perfectly good field. Such a value can only repeat what the entry
    /// already said, so dropping it costs nothing when the block is well formed, and it is the
    /// only way a malformed block can be told apart from a meaningful one.
    ///
    /// The `None` fields this leaves behind are the same `None` the writer uses for "this entry
    /// has nothing to record here", so a block that was ignored on the way in is not written back
    /// out on the way through.
    ///
    /// `entry_header_start` is `None` for a local header, which has no relative offset field for
    /// the block to override in the first place.
    #[inline]
    pub(crate) fn parse<R: Read>(
        reader: &mut R,
        len: u16,
        entry_uncompressed_size: u32,
        entry_compressed_size: u32,
        entry_header_start: Option<u32>,
    ) -> ZipResult<Self> {
        let mut consumed_len = 0;

        let uncompressed_size = Self::read_field(
            reader,
            len,
            &mut consumed_len,
            u64::from(entry_uncompressed_size) == ZIP64_BYTES_THR,
        )?;
        let compressed_size = Self::read_field(
            reader,
            len,
            &mut consumed_len,
            u64::from(entry_compressed_size) == ZIP64_BYTES_THR,
        )?;
        let header_start = Self::read_field(
            reader,
            len,
            &mut consumed_len,
            entry_header_start.is_some_and(|start| u64::from(start) == ZIP64_BYTES_THR),
        )?;

        // The two sizes travel together, so one sentinel brings both along. The field that had no
        // sentinel keeps the entry's own value, which is what it already held.
        let sizes =
            (uncompressed_size.is_some() || compressed_size.is_some()).then(|| Zip64Sizes {
                uncompressed_size: uncompressed_size
                    .unwrap_or_else(|| entry_uncompressed_size.into()),
                compressed_size: compressed_size.unwrap_or_else(|| entry_compressed_size.into()),
            });

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

        Ok(Self {
            sizes,
            header_start,
        })
    }
}
