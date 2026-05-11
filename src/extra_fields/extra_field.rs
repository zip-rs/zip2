//! Code related to the `ExtraField` enum

use crate::AesMode;
use crate::CompressionMethod;
use crate::extra_fields::AexEncryption;
use crate::extra_fields::ExtendedTimestamp;
use crate::extra_fields::Ntfs;
use crate::extra_fields::UnicodeExtraField;
use crate::extra_fields::UsedExtraField;
use crate::extra_fields::Zip64ExtendedInformation;
use crate::spec::ZipLocalEntryBlock;
use crate::format::flags::ZipFlags;
use crate::result::ZipResult;
use crate::result::invalid;
use crate::types::AesVendorVersion;
use crate::types::ZipFileData;
use crate::unstable::LittleEndianReadExt;
use std::io::Cursor;
use std::io::ErrorKind;
use std::io::Read;

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
    },
    /// Zip64 Information
    Zip64ExtendedInformation {
        /// uncompressed size
        uncompressed_size: u64,
        /// compressed size
        compressed_size: u64,
        /// header start
        header_start: u64,
    },
    /// Unicode Comment
    UnicodeComment(UnicodeExtraField),
    /// UnicodePath
    UnicodePath(UnicodeExtraField),
    DataStreamAlignment(u64),
    CustomExtraField {
        local: bool,
        header_id: u16,
        data: Vec<u8>,
    },
    /// Unknown
    Unknown(Vec<u8>),
}

#[derive(Debug, Clone, Default)]
pub struct ExtraFields {
    pub(crate) inner: Vec<ExtraField>
}

impl ExtraFields {
    pub(crate) fn new() -> Self {
        Self {
            inner: Vec::new()
        }
    }

    pub(crate) fn strip_alignment_extra_field(&mut self, remove_zip64: bool) -> ExtraFields {
        self.inner.retain(|extra| {
            if remove_zip64 {
                matches!(extra, ExtraField::DataStreamAlignment(_))
            } else {
                matches!(extra, ExtraField::DataStreamAlignment(_) | ExtraField::Zip64ExtendedInformation { ..})
            }
        });
        self
    }
    pub(crate) fn parse<R: Read>(
        buff: &[u8],
        block: &ZipLocalEntryBlock,
        file: &mut ZipFileData,
        file_name_raw: &mut Vec<u8>,
    ) -> ZipResult<Self> {
        let mut reader = Cursor::new(buff);
        let mut extra_fields = Vec::new();
        while reader.position() < buff.len() {
            let parsed_extra_field = ExtraField::parse(&mut reader, block)?;
            parsed_extra_field.use_with(file, file_name_raw);
            extra_fields.push(parsed_extra_field);
        }
        Ok(Self {
            inner: extra_fields
        })
    }
}

impl ExtraField {
    pub(crate) fn parse<R: Read>(reader: &mut R, file: &ZipLocalEntryBlock) -> ZipResult<Self> {
        let kind = match reader.read_u16_le() {
            Ok(kind) => kind,
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let decoded_extra_field = UsedExtraField::try_from(kind);
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
                    file.uncompressed_size,
                    file.compressed_size,
                    file.header_start,
                )?;
                ExtraField::Zip64ExtendedInformation {
                    uncompressed_size: new_uncomp,
                    compressed_size: new_comp,
                    header_start: new_head,
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
                ExtraField::Unknown(buf)
                // Other fields are ignored
            }
        };
        Ok(parsed_extra_field)
    }

    pub(crate) fn use_with(&self, file: &mut ZipFileData, file_name_raw: &mut Vec<u8>) {
        match *self {
            // Zip64 extended information extra field
            ExtraField::Zip64ExtendedInformation {
                uncompressed_size,
                compressed_size,
                header_start,
            } => {
                file.large_file = true;
                file.uncompressed_size = uncompressed_size;
                file.compressed_size = compressed_size;
                file.header_start = header_start;
                return Ok(true);
            }
            ExtraField::Ntfs(ntfs) => {
                // NTFS extra field
            }
            ExtraField::AeXEncryption {
                aes_mode,
                aes_vendor_version,
                compression_method,
            } => {
                file.aes_mode = Some((aes_mode, aes_vendor_version));
                file.compression_method = compression_method;
            }
            ExtraField::ExtendedTimestamp(extended_timestamp) => {
                // nothing to do
            }
            ExtraField::UnicodeComment(unicode_comment) => {
                // Info-ZIP Unicode Comment Extra Field
                // APPNOTE 4.6.8 and https://libzip.org/specifications/extrafld.txt
                file.file_comment = String::from_utf8(
                    unicode_comment
                        .unwrap_valid(file.file_comment.as_bytes())?
                        .into_vec(),
                )?
                .into();
            }
            ExtraField::UnicodePath(unicode_path) => {
                // Info-ZIP Unicode Path Extra Field
                // APPNOTE 4.6.9 and https://libzip.org/specifications/extrafld.txt
                let file_name = unicode_path.unwrap_valid(file_name_raw)?;
                *file_name_raw = file_name.into_vec();
                file.flags |= ZipFlags::LanguageEncoding.as_u16();
            }
            ExtraField::Unknown(_) => {}
        }
    }
}
