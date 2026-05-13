//! Code related to the `ExtraField` enum

use crate::AesMode;
use crate::CompressionMethod;
use crate::extra_fields::AexEncryption;
use crate::extra_fields::CustomExtraField;
use crate::extra_fields::ExtendedTimestamp;
use crate::extra_fields::Ntfs;
use crate::extra_fields::UnicodeExtraField;
use crate::extra_fields::UsedExtraField;
use crate::extra_fields::Zip64ExtendedInformation;
use crate::format::flags::ZipFlags;
use crate::result::ZipResult;
use crate::result::invalid;
use crate::spec::BlockGetter;
use crate::types::AesVendorVersion;
use crate::types::ZipFileData;
use crate::unstable::LittleEndianReadExt;
use core::mem;
use std::io::ErrorKind;
use std::io::{Cursor, Read, Write};

/// contains one extra field
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ExtraField {
    /// NTFS extra field
    Ntfs(Ntfs),

    /// extended timestamp, as described in <https://libzip.org/specifications/extrafld.txt>
    ExtendedTimestamp(ExtendedTimestamp),

    /// AeX Encryption
    AeXEncryption {
        /// AES mode
        aes_mode: AesMode,
        /// AES vendor version
        aes_vendor_version: AesVendorVersion,
        /// compression method
        compression_method: CompressionMethod,
        /// aes extra field start in local header
        aes_extra_field_start: Option<usize>,
    },
    /// Zip64 Information
    Zip64ExtendedInformation {
        /// uncompressed size
        uncompressed_size: Option<u64>,
        /// compressed size
        compressed_size: Option<u64>,
        /// header start
        header_start: Option<u64>,
    },
    /// Unicode Comment
    UnicodeComment(UnicodeExtraField),
    /// UnicodePath
    UnicodePath(UnicodeExtraField),
    /// Data Stream Alignment
    DataStreamAlignment(u64),
    /// Custom extra field
    Custom(CustomExtraField),
    /// No Op extra field
    NoOp,
}

/// Extra fields list
#[derive(Debug, Clone, Default)]
pub struct ExtraFields {
    pub(crate) inner: Vec<ExtraField>,
}

impl ExtraFields {
    pub(crate) fn new() -> Self {
        Self { inner: Vec::new() }
    }
    /*
        pub(crate) fn strip_alignment_extra_field(&mut self, remove_zip64: bool) {
            self.inner.retain(|extra| {
                if remove_zip64 {
                    matches!(extra, ExtraField::DataStreamAlignment(_))
                } else {
                    matches!(
                        extra,
                        ExtraField::DataStreamAlignment(_)
                            | ExtraField::Zip64ExtendedInformation { .. }
                    )
                }
            });
        }
    */
    pub(crate) fn parse<B: BlockGetter>(buff: &[u8], block: &B) -> ZipResult<Self> {
        let mut reader = Cursor::new(buff);
        let mut extra_fields = Vec::new();
        while (reader.position() as usize) < buff.len() {
            let parsed_extra_field = ExtraField::parse(&mut reader, block)?;
            let Some(parsed_extra_field) = parsed_extra_field else {
                break;
            };
            extra_fields.push(parsed_extra_field);
        }
        Ok(Self {
            inner: extra_fields,
        })
    }

    pub(crate) fn local_extra_fields_mut(&mut self) -> std::slice::IterMut<'_, ExtraField> {
        self.inner.iter_mut()
    }

    pub(crate) fn local_extra_fields(&self) -> std::slice::Iter<'_, ExtraField> {
        self.inner.iter()
    }

    pub(crate) fn central_extra_fields(&self) -> std::slice::Iter<'_, ExtraField> {
        self.inner.iter()
    }
}

impl ExtraField {
    pub(crate) fn parse<R: Read, B: BlockGetter>(
        reader: &mut R,
        file: &B,
    ) -> ZipResult<Option<Self>> {
        let extra_field_header_id = match reader.read_u16_le() {
            Ok(value) => value,
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let decoded_extra_field = UsedExtraField::try_from(extra_field_header_id);
        let len = match decoded_extra_field {
            Ok(known_field) => match reader.read_u16_le() {
                Ok(len) => len,
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                    return Err(invalid!("Extra field {} header truncated", known_field));
                }
                Err(e) => return Err(e.into()),
            },
            Err(()) => {
                match reader.read_u16_le() {
                    Ok(len) => len,
                    Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(None), // early return, most likely a padding
                    Err(_e) => {
                        // Consume remaining bytes to avoid infinite loop in caller
                        let mut buf = Vec::new();
                        let _ = reader.read_to_end(&mut buf);
                        return Ok(None);
                    }
                }
            }
        };
        let parsed_extra_field = match decoded_extra_field {
            // Zip64 extended information extra field
            Ok(UsedExtraField::Zip64ExtendedInfo) => {
                let (new_uncomp, new_comp, new_head) = Zip64ExtendedInformation::parse(
                    reader,
                    len,
                    file.get_uncompressed_size(),
                    file.get_compressed_size(),
                    file.get_header_start(),
                )?;
                ExtraField::Zip64ExtendedInformation {
                    uncompressed_size: Some(new_uncomp),
                    compressed_size: Some(new_comp),
                    header_start: Some(new_head),
                }
            }
            Ok(UsedExtraField::Ntfs) => {
                // NTFS extra field
                ExtraField::Ntfs(Ntfs::try_from_reader(reader, len)?)
            }
            Ok(UsedExtraField::AeXEncryption) => {
                // AES
                let (new_aes_enc, inner_compression) = AexEncryption::parse(reader, len)?;
                ExtraField::AeXEncryption {
                    aes_mode: new_aes_enc.0,
                    aes_vendor_version: new_aes_enc.1,
                    compression_method: inner_compression,
                    aes_extra_field_start: None,
                }
            }
            Ok(UsedExtraField::ExtendedTimestamp) => {
                ExtraField::ExtendedTimestamp(ExtendedTimestamp::try_from_reader(reader, len)?)
            }
            Ok(UsedExtraField::UnicodeComment) => {
                // Info-ZIP Unicode Comment Extra Field
                // APPNOTE 4.6.8 and https://libzip.org/specifications/extrafld.txt
                let unicode = UnicodeExtraField::try_from_reader(reader, len)?;
                ExtraField::UnicodeComment(unicode)
            }
            Ok(UsedExtraField::UnicodePath) => {
                // Info-ZIP Unicode Path Extra Field
                // APPNOTE 4.6.9 and https://libzip.org/specifications/extrafld.txt
                let unicode = UnicodeExtraField::try_from_reader(reader, len)?;
                ExtraField::UnicodePath(unicode)
            }
            _ => {
                let mut buf = vec![0u8; len as usize];
                if let Err(e) = reader.read_exact(&mut buf) {
                    if e.kind() == ErrorKind::UnexpectedEof {
                        return Err(invalid!("Extra field content truncated"));
                    }
                    return Err(e.into());
                }
                ExtraField::Custom(CustomExtraField::new(false, extra_field_header_id, &buf))
                // Other fields are ignored
            }
        };
        Ok(Some(parsed_extra_field))
    }

    pub(crate) fn size(&self, is_local_header: bool) -> usize {
        match self {
            // Zip64 extended information extra field
            ExtraField::Zip64ExtendedInformation {
                uncompressed_size,
                compressed_size,
                header_start,
            } => {
                let mut size = 0;
                if uncompressed_size.is_some() {
                    size += mem::size_of::<u64>();
                }
                if compressed_size.is_some() {
                    size += mem::size_of::<u64>();
                }
                if !is_local_header && header_start.is_some() {
                    size += mem::size_of::<u64>();
                }
                size + mem::size_of::<UsedExtraField>() + mem::size_of::<u16>()
            }
            ExtraField::Ntfs(ntfs) => {
                // NTFS extra field
                0
            }
            ExtraField::AeXEncryption { .. } => AexEncryption::FULL_SIZE,
            ExtraField::ExtendedTimestamp(extended_timestamp) => {
                // nothing to do
                0
            }
            ExtraField::UnicodeComment(unicode_comment) => unicode_comment.full_size(),
            ExtraField::UnicodePath(unicode_path) => unicode_path.full_size(),
            ExtraField::Custom(custom) => custom.len(),
            _ => 0,
        }
    }

    pub(crate) fn write<W: Write>(&self, writer: &mut W, is_local_header: bool) -> ZipResult<()> {
        match self {
            // Zip64 extended information extra field
            ExtraField::Zip64ExtendedInformation {
                uncompressed_size,
                compressed_size,
                header_start,
            } => {
                // TODO
                if is_local_header {}
            }
            ExtraField::AeXEncryption {
                aes_mode,
                aes_vendor_version,
                compression_method,
                ..
            } => {
                let aex = AexEncryption::new(*aes_vendor_version, *aes_mode, *compression_method);
                aex.write(writer)?;
            }
            ExtraField::Custom(custom) => {
                custom.write(writer)?;
            }
            ExtraField::UnicodeComment(unicode_comment) => {
                let magic = UsedExtraField::UnicodeComment.as_u16();
                writer.write_all(&magic.to_le_bytes())?;
                unicode_comment.write(writer)?;
            }
            ExtraField::UnicodePath(unicode_path) => {
                let magic = UsedExtraField::UnicodePath.as_u16();
                writer.write_all(&magic.to_le_bytes())?;
                unicode_path.write(writer)?;
            }
            _ => {
                // nothing to do
            }
        }
        Ok(())
    }
}

impl ZipFileData {
    pub(crate) fn apply_extra_fields(&mut self, mut file_name_raw: &mut Vec<u8>) -> ZipResult<()> {
        for one_extra_field in &self.extra_fields.inner {
            match one_extra_field {
                // Zip64 extended information extra field
                ExtraField::Zip64ExtendedInformation {
                    uncompressed_size,
                    compressed_size,
                    header_start,
                } => {
                    self.large_file = true;
                    if let Some(uncomp_size) = *uncompressed_size {
                        self.uncompressed_size = uncomp_size;
                    }
                    if let Some(comp_size) = *compressed_size {
                        self.compressed_size = comp_size;
                    }
                    if let Some(head_start) = *header_start {
                        self.header_start = head_start;
                    }
                }
                ExtraField::AeXEncryption {
                    aes_mode,
                    aes_vendor_version,
                    compression_method,
                    ..
                } => {
                    self.aes_mode = Some((*aes_mode, *aes_vendor_version));
                    self.compression_method = *compression_method;
                }
                ExtraField::UnicodeComment(unicode_comment) => {
                    // Info-ZIP Unicode Comment Extra Field
                    // APPNOTE 4.6.8 and https://libzip.org/specifications/extrafld.txt
                    self.file_comment = String::from_utf8(
                        unicode_comment
                            .unwrap_valid(self.file_comment.as_bytes())?
                            .into_vec(),
                    )?
                    .into();
                }
                ExtraField::UnicodePath(unicode_path) => {
                    // Info-ZIP Unicode Path Extra Field
                    // APPNOTE 4.6.9 and https://libzip.org/specifications/extrafld.txt
                    let file_name = unicode_path.unwrap_valid(file_name_raw)?;
                    *file_name_raw = file_name.into_vec();
                    self.flags |= ZipFlags::LanguageEncoding.as_u16();
                }
                _ => {
                    // nothing to do
                }
            }
        }
        Ok(())
    }
}
