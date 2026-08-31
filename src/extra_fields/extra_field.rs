//! Code related to the `ExtraField` enum

use crate::ZIP64_BYTES_THR;
#[cfg(feature = "aes-crypto")]
use crate::extra_fields::AexEncryption;
use crate::extra_fields::CustomExtraField;
use crate::extra_fields::DataStreamAlignment;
use crate::extra_fields::ExtendedTimestamp;
use crate::extra_fields::Ntfs;
use crate::extra_fields::UnicodeExtraField;
use crate::extra_fields::UsedExtraField;
use crate::extra_fields::Zip64ExtendedInformation;
use crate::extra_fields::zip64_extended_information::Zip64Sizes;
use crate::format::ZIP64_BYTES_THR_U32;
use crate::format::blocks::ZipEntryBlock;
use crate::format::flags::ZipFlags;
use crate::result::ZipResult;
use crate::result::invalid;
use crate::types::ZipFileData;
use crate::unstable::LittleEndianReadExt;
use std::io::ErrorKind;
use std::io::{Cursor, Read, Write};

/// contains one extra field
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ExtraField {
    /// NTFS extra field
    Ntfs(Ntfs),
    /// extended timestamp, as described in <https://libzip.org/specifications/extrafld.txt>
    ExtendedTimestamp(ExtendedTimestamp),
    /// AeX Encryption
    #[cfg(feature = "aes-crypto")]
    AeXEncryption(AexEncryption),
    /// Zip64 Information
    Zip64ExtendedInformation(Zip64ExtendedInformation),
    /// Unicode Comment
    UnicodeComment(UnicodeExtraField),
    /// UnicodePath
    UnicodePath(UnicodeExtraField),
    /// Data Stream Alignment
    DataStreamAlignment(DataStreamAlignment),
    /// Custom extra field
    Custom(CustomExtraField),
}

/// Extra fields list
#[derive(Debug, Clone, Default)]
pub struct ExtraFields {
    pub(crate) inner: Vec<ExtraField>,
}

impl ExtraFields {
    pub(crate) fn parse<B: ZipEntryBlock>(buff: &[u8], block: &B) -> ZipResult<Self> {
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

    pub(crate) fn local_extra_fields_mut(&mut self) -> impl Iterator<Item = &mut ExtraField> {
        self.inner.iter_mut()
    }

    pub(crate) fn local_extra_fields(&self) -> impl Iterator<Item = &ExtraField> {
        self.inner.iter().filter(|ef| match ef {
            ExtraField::Custom(cef) => !cef.central_only,
            _ => true,
        })
    }

    pub(crate) fn central_extra_fields(&self) -> impl Iterator<Item = &ExtraField> {
        // data alignment is local only
        self.inner
            .iter()
            .filter(|ef| !matches!(ef, ExtraField::DataStreamAlignment(_)))
    }
}

impl ExtraField {
    pub(crate) fn parse<R: Read, B: ZipEntryBlock>(
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
                        let _ = std::io::copy(reader, &mut std::io::sink());
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
                ExtraField::Zip64ExtendedInformation(Zip64ExtendedInformation {
                    sizes: Some(Zip64Sizes {
                        uncompressed_size: new_uncomp,
                        compressed_size: new_comp,
                    }),
                    header_start: Some(new_head),
                })
            }
            Ok(UsedExtraField::Ntfs) => {
                // NTFS extra field
                ExtraField::Ntfs(Ntfs::try_from_reader(reader, len)?)
            }
            #[cfg(feature = "aes-crypto")]
            Ok(UsedExtraField::AeXEncryption) => {
                // AES
                let (new_aes_enc, inner_compression) = AexEncryption::parse(reader, len)?;
                ExtraField::AeXEncryption(AexEncryption::new(
                    new_aes_enc.1,
                    new_aes_enc.0,
                    inner_compression,
                ))
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
                ExtraField::Custom(CustomExtraField::new(
                    false,
                    extra_field_header_id,
                    buf.into_boxed_slice(),
                ))
                // Other fields are ignored
            }
        };
        Ok(Some(parsed_extra_field))
    }

    pub(crate) fn size(&self, is_local_header: bool) -> usize {
        match self {
            // Zip64 extended information extra field
            ExtraField::Zip64ExtendedInformation(zip64_extra) => {
                zip64_extra.full_size(is_local_header)
            }
            ExtraField::Ntfs(_ntfs) => {
                // NTFS extra field
                0
            }
            #[cfg(feature = "aes-crypto")]
            ExtraField::AeXEncryption(aes) => aes.full_size(),
            ExtraField::ExtendedTimestamp(_extended_timestamp) => {
                // nothing to do
                0
            }
            ExtraField::UnicodeComment(unicode_comment) => unicode_comment.full_size(),
            ExtraField::UnicodePath(unicode_path) => unicode_path.full_size(),
            ExtraField::Custom(custom) => custom.full_size(is_local_header),
            ExtraField::DataStreamAlignment(data_stream_alignment) => {
                data_stream_alignment.full_size(is_local_header)
            }
        }
    }

    pub(crate) fn write<W: Write>(&self, writer: &mut W, is_local_header: bool) -> ZipResult<()> {
        match self {
            // Zip64 extended information extra field
            ExtraField::Zip64ExtendedInformation(zip64_extra) => {
                zip64_extra.write(writer, is_local_header)?;
            }
            #[cfg(feature = "aes-crypto")]
            ExtraField::AeXEncryption(aex) => {
                aex.write(writer)?;
            }
            ExtraField::Custom(custom) => {
                custom.write(writer, is_local_header)?;
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
            ExtraField::DataStreamAlignment(data_stream_alignment) => {
                data_stream_alignment.write(writer, is_local_header)?
            }
            _ => {
                // nothing to do
            }
        }
        Ok(())
    }
}

impl ZipFileData {
    pub(crate) fn apply_extra_fields(&mut self, file_name_raw: &mut Vec<u8>) -> ZipResult<()> {
        for one_extra_field in &self.extra_fields.inner {
            match one_extra_field {
                // Zip64 extended information extra field
                ExtraField::Zip64ExtendedInformation(zip64_block) => {
                    self.large_file = true;
                    if (self.uncompressed_size >= ZIP64_BYTES_THR
                        || self.compressed_size >= ZIP64_BYTES_THR)
                        && let Some(Zip64Sizes {
                            uncompressed_size,
                            compressed_size,
                        }) = zip64_block.sizes
                    {
                        self.uncompressed_size = uncompressed_size;
                        self.compressed_size = compressed_size;
                    }
                    if self.header_start >= ZIP64_BYTES_THR
                        && let Some(head_start) = zip64_block.header_start
                    {
                        self.header_start = head_start;
                    }
                }
                #[cfg(feature = "aes-crypto")]
                ExtraField::AeXEncryption(AexEncryption {
                    compression_method, ..
                }) => {
                    self.compression_method = *compression_method;
                }
                ExtraField::UnicodeComment(unicode) => {
                    // Info-ZIP Unicode Comment Extra Field
                    // APPNOTE 4.6.8 and https://libzip.org/specifications/extrafld.txt
                    // If the CRC check fails, this Unicode Comment extra field SHOULD be ignored and
                    // the File Comment field in the header SHOULD be used instead.
                    // Check if the comment is UTF-8
                    if unicode.is_crc32_valid(self.file_comment.as_bytes())
                        && let Ok(comment) = String::from_utf8(unicode.content.to_vec())
                    {
                        self.file_comment = comment.into_boxed_str();
                    }
                }
                #[allow(clippy::collapsible_match)]
                ExtraField::UnicodePath(unicode) => {
                    // Info-ZIP Unicode Path Extra Field
                    // APPNOTE 4.6.9 and https://libzip.org/specifications/extrafld.txt
                    // If the CRC check fails, this UTF-8 Path Extra Field SHOULD be ignored and
                    // the File Name field in the header SHOULD be used instead.
                    if unicode.is_crc32_valid(file_name_raw)
                        && std::str::from_utf8(&unicode.content).is_ok()
                    {
                        *file_name_raw = unicode.content.to_vec();
                        self.flags |= ZipFlags::LanguageEncoding.as_u16();
                    }
                }
                _ => {
                    // nothing to do
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::extra_fields::ExtraField;
    use crate::extra_fields::ExtraFields;
    use crate::format::blocks::ZipEntryBlock;

    struct PlaceHolderBlock;

    impl ZipEntryBlock for PlaceHolderBlock {
        fn get_uncompressed_size(&self) -> u32 {
            0
        }
        fn get_compressed_size(&self) -> u32 {
            0
        }
        fn get_header_start(&self) -> Option<u32> {
            None
        }
    }

    #[test]
    #[cfg(feature = "aes-crypto")]
    fn aex_extra_field_with_feature() {
        let buff = [1, 0x99, 7, 0, 1, 0, b'A', b'E', 3, 0, 0];

        let extra_fields = ExtraFields::parse(&buff[..], &PlaceHolderBlock).unwrap();
        assert!(matches!(
            extra_fields.inner[0],
            ExtraField::AeXEncryption(..)
        ));
    }

    #[test]
    #[cfg(not(feature = "aes-crypto"))]
    fn aex_extra_field_without_feature() {
        use crate::extra_fields::CustomExtraField;
        use crate::extra_fields::UsedExtraField;

        let buff = [1, 0x99, 7, 0, 1, 0, b'A', b'E', 3, 0, 0];

        let extra_fields = ExtraFields::parse(&buff[..], &PlaceHolderBlock).unwrap();
        let extra = CustomExtraField::new(
            false,
            UsedExtraField::AeXEncryption.as_u16(),
            [1, 0, b'A', b'E', 3, 0, 0].to_vec().into_boxed_slice(),
        );
        assert_eq!(extra_fields.inner[0], ExtraField::Custom(extra));
    }
}
