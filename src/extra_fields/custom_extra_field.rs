//! Custom extra field

use crate::result::ZipResult;
use crate::result::invalid;
use std::io::Write;

/// A Custom Extra Field
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CustomExtraField {
    /// If true, this field will be included in the central directory entry but not the local file header.
    pub(crate) central_only: bool,
    /// Optional alternate payload for the central directory when the central representation
    /// differs from the local header (for example, Extended Timestamp `0x5455`, which omits
    /// access and creation times in the central directory).
    pub(crate) central_data: Option<Box<[u8]>>,
    /// Header ID of the extra field
    pub header_id: u16,
    /// Data of the extra field
    pub data: Box<[u8]>,
}

impl CustomExtraField {
    pub(crate) fn new(central_only: bool, header_id: u16, data: Box<[u8]>) -> Self {
        Self {
            central_only,
            central_data: None,
            header_id,
            data,
        }
    }

    pub(crate) fn new_with_central_data(
        header_id: u16,
        local_data: Box<[u8]>,
        central_data: Box<[u8]>,
    ) -> Self {
        Self {
            central_only: false,
            central_data: Some(central_data),
            header_id,
            data: local_data,
        }
    }

    #[allow(unused)] // used for tests
    pub(crate) fn new_from_raw(central_only: bool, data: &[u8]) -> ZipResult<Self> {
        if data.len() < 2 {
            return Err(invalid!("Cannot build a CustomExtraField: no header_id"));
        }
        if data.len() < 4 {
            return Err(invalid!("Cannot build a CustomExtraField: no size"));
        }
        let header_id = u16::from_le_bytes([data[0], data[1]]);
        let size = u16::from_le_bytes([data[2], data[3]]) as usize;
        if size > (u16::MAX - 4) as usize {
            return Err(invalid!("Cannot build a CustomExtraField: size too big"));
        }
        let data_rest = &data[4..];
        if size != data_rest.len() {
            return Err(invalid!("Cannot build a CustomExtraField: incorrect size"));
        }
        Ok(Self {
            central_only,
            central_data: None,
            header_id,
            data: data_rest.to_vec().into_boxed_slice(),
        })
    }

    /// The number of bytes this field occupies in the local file header or central directory entry.
    #[must_use]
    pub fn full_size(&self, is_local_header: bool) -> usize {
        if self.central_only && is_local_header {
            return 0;
        }
        let payload = if is_local_header {
            &self.data
        } else {
            self.central_data.as_deref().unwrap_or(&self.data)
        };
        size_of::<u16>() + size_of::<u16>() + payload.len()
    }

    pub(crate) fn write<W: Write>(&self, write: &mut W, is_local_header: bool) -> ZipResult<()> {
        if self.central_only && is_local_header {
            return Ok(());
        }
        let payload = if is_local_header {
            &self.data
        } else {
            self.central_data.as_deref().unwrap_or(&self.data)
        };
        write.write_all(&self.header_id.to_le_bytes())?;
        let size =
            u16::try_from(payload.len()).map_err(|_| invalid!("Extra data field is too large"))?;
        write.write_all(&size.to_le_bytes())?;
        write.write_all(payload)?;
        Ok(())
    }
}

#[cfg(feature = "_arbitrary")]
impl arbitrary::Arbitrary<'_> for CustomExtraField {
    fn arbitrary(u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<Self> {
        use crate::ExtraField;
        use crate::format::blocks::{Pod, ZipLocalEntryBlock};
        use std::io::Cursor;
        let extra_field = CustomExtraField {
            central_only: u.arbitrary()?,
            central_data: None,
            header_id: u.arbitrary()?,
            data: u.arbitrary()?,
        };
        let mut serialized_extra_field = Vec::with_capacity(extra_field.data.len() + 4);
        if extra_field
            .write(&mut serialized_extra_field, true)
            .is_err()
        {
            return Err(arbitrary::Error::IncorrectFormat);
        }
        let block = ZipLocalEntryBlock::zeroed();
        if ExtraField::parse(&mut Cursor::new(&serialized_extra_field), &block, true).is_err() {
            Err(arbitrary::Error::IncorrectFormat)
        } else {
            Ok(extra_field)
        }
    }
}
