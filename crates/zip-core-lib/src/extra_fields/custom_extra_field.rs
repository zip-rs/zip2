//! Custom extra field

use crate::result::ZipResult;
use crate::result::invalid;
use std::io::Write;

/// A Custom Extra Field
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CustomExtraField {
    /// If true, this field will be included in the central directory entry but not the local file header.
    pub central_only: bool,
    /// Header ID of the extra field
    pub header_id: u16,
    /// Data of the extra field
    pub data: Box<[u8]>,
}

impl CustomExtraField {
    pub fn new(central_only: bool, header_id: u16, data: Box<[u8]>) -> Self {
        Self {
            central_only,
            header_id,
            data,
        }
    }

    #[allow(unused)] // used for tests
    pub fn new_from_raw(central_only: bool, data: &[u8]) -> ZipResult<Self> {
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
            header_id,
            data: data_rest.to_vec().into_boxed_slice(),
        })
    }

    pub fn full_size(&self, is_local_header: bool) -> usize {
        if self.central_only && is_local_header {
            return 0;
        }
        let size = self.data.len();
        size_of::<u16>() + size_of::<u16>() + size
    }

    pub fn write<W: Write>(&self, write: &mut W, is_local_header: bool) -> ZipResult<()> {
        if self.central_only && is_local_header {
            return Ok(());
        }
        write.write_all(&self.header_id.to_le_bytes())?;
        let size = self.data.len() as u16;
        write.write_all(&size.to_le_bytes())?;
        write.write_all(&self.data)?;
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
