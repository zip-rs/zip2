//! Types for reading ZIP archives

use crate::compression::CompressionMethod;
use crate::cp437::FromCp437;
use crate::datetime::DateTime;
use crate::extra_fields::{ExtraField, ExtraFields};
use crate::format::blocks::{CentralDirectoryEndInfo, DataAndPosition, ZipCentralEntryBlock};
use crate::format::flags::{ZipFileFlags, ZipFlags};
use crate::format::system::System;
use crate::result::{ZipError, ZipResult, invalid};
use crate::types::ZipFileData;
use indexmap::IndexMap;
use std::io::{self, Read, Seek};
use std::sync::OnceLock;

mod config;
pub use config::{ArchiveOffset, Config};

pub(crate) mod extract;
pub use extract::{ExtractOptions, RootDirFilter, SymlinkExtractAction, root_dir_common_filter};

/// Provides high level API for reading from a stream.
pub(crate) mod stream;
pub use stream::{
    read_zipfile_from_stream, read_zipfile_from_stream_with_compressed_size,
    read_zipfile_from_stream_with_options,
};

pub(crate) mod magic_finder;
pub(crate) mod readers;

pub(crate) mod zipfile;
pub use zipfile::{ZipFile, ZipFileEntry, ZipFileEntryWithDataDescriptor, ZipFileSeek};

pub(crate) mod zip_archive;
pub use zip_archive::{ZipArchive, ZipArchiveMetadata};

#[cfg(feature = "aes-crypto")]
pub use crate::aes::AesInfo;

#[derive(Debug)]
pub(crate) struct CentralDirectoryInfo {
    pub(crate) archive_offset: u64,
    pub(crate) directory_start: u64,
    pub(crate) number_of_files: usize,
    pub(crate) disk_number: u32,
    pub(crate) disk_with_central_directory: u32,
}

impl<'a> TryFrom<&'a CentralDirectoryEndInfo> for CentralDirectoryInfo {
    type Error = ZipError;

    fn try_from(value: &'a CentralDirectoryEndInfo) -> Result<Self, Self::Error> {
        let (relative_cd_offset, number_of_files, disk_number, disk_with_central_directory) =
            match &value.eocd64 {
                Some(DataAndPosition { data: eocd64, .. }) => {
                    if eocd64.number_of_files_on_this_disk > eocd64.number_of_files {
                        return Err(invalid!(
                            "ZIP64 footer indicates more files on this disk than in the whole archive"
                        ));
                    }
                    (
                        eocd64.central_directory_offset,
                        eocd64.number_of_files as usize,
                        eocd64.disk_number,
                        eocd64.disk_with_central_directory,
                    )
                }
                _ => (
                    u64::from(value.eocd.data.central_directory_offset),
                    value.eocd.data.number_of_files_on_this_disk as usize,
                    u32::from(value.eocd.data.disk_number),
                    u32::from(value.eocd.data.disk_with_central_directory),
                ),
            };

        let directory_start = relative_cd_offset
            .checked_add(value.archive_offset)
            .ok_or(invalid!("Invalid central directory size or offset"))?;

        Ok(Self {
            archive_offset: value.archive_offset,
            directory_start,
            number_of_files,
            disk_number,
            disk_with_central_directory,
        })
    }
}

impl<R: Read + Seek> ZipArchive<R> {
    pub(crate) fn merge_contents<W: std::io::Write + Seek>(
        &mut self,
        mut w: W,
    ) -> ZipResult<IndexMap<Box<[u8]>, ZipFileData>> {
        if self.shared.files.is_empty() {
            return Ok(IndexMap::new());
        }
        let mut new_files = self.shared.files.clone();
        /* The first file header will probably start at the beginning of the file, but zip doesn't
         * enforce that, and executable zips like PEX files will have a shebang line so will
         * definitely be greater than 0.
         *
         * assert_eq!(0, new_files[0].header_start); // Avoid this.
         */

        let first_new_file_header_start = w.stream_position()?;

        /* Push back file header starts for all entries in the covered files. */
        new_files.values_mut().try_for_each(|f| {
            /* This is probably the only really important thing to change. */
            f.header_start = f
                .header_start
                .checked_add(first_new_file_header_start)
                .ok_or(invalid!(
                    "new header start from merge would have been too large"
                ))?;
            /* This is only ever used internally to cache metadata lookups (it's not part of the
             * zip spec), and 0 is the sentinel value. */
            f.central_header_start = 0;
            /* This is an atomic variable so it can be updated from another thread in the
             * implementation (which is good!). */
            if let Some(old_data_start) = f.data_start.take() {
                let new_data_start = old_data_start
                    .checked_add(first_new_file_header_start)
                    .ok_or(invalid!(
                        "new data start from merge would have been too large"
                    ))?;
                f.data_start.get_or_init(|| new_data_start);
            }
            Ok::<_, ZipError>(())
        })?;

        /* Rewind to the beginning of the file.
         *
         * NB: we *could* decide to start copying from new_files[0].header_start instead, which
         * would avoid copying over e.g. any pex shebangs or other file contents that start before
         * the first zip file entry. However, zip files actually shouldn't care about garbage data
         * in *between* real entries, since the central directory header records the correct start
         * location of each, and keeping track of that math is more complicated logic that will only
         * rarely be used, since most zips that get merged together are likely to be produced
         * specifically for that purpose (and therefore are unlikely to have a shebang or other
         * preface). Finally, this preserves any data that might actually be useful.
         */
        self.reader.rewind()?;
        /* Find the end of the file data. */
        let length_to_read = self.shared.dir_start;
        /* Produce a Read that reads bytes up until the start of the central directory header.
         * This "as &mut dyn Read" trick is used elsewhere to avoid having to clone the underlying
         * handle, which it really shouldn't need to anyway. */
        let mut limited_raw = (&mut self.reader as &mut dyn Read).take(length_to_read);
        /* Copy over file data from source archive directly. */
        io::copy(&mut limited_raw, &mut w)?;

        /* Return the files we've just written to the data stream. */
        Ok(new_files)
    }
}

#[inline]
fn read_variable_length_byte_field<R: Read>(reader: &mut R, len: usize) -> ZipResult<Vec<u8>> {
    let mut data = vec![0; len];
    if let Err(e) = reader.read_exact(&mut data) {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            return Err(invalid!(
                "Variable-length field extends beyond file boundary"
            ));
        }
        return Err(e.into());
    }
    Ok(data)
}

/// Parse a central directory entry to collect the information for the file.
fn central_header_to_zip_file_inner<R: Read>(
    reader: &mut R,
    archive_offset: u64,
    central_header_start: u64,
    block: ZipCentralEntryBlock,
) -> ZipResult<(ZipFileData, Vec<u8>)> {
    let ZipCentralEntryBlock {
        // magic,
        version_made_by,
        // version_to_extract,
        flags,
        compression_method,
        last_mod_time,
        last_mod_date,
        crc32,
        compressed_size,
        uncompressed_size,
        file_name_length,
        extra_field_length,
        file_comment_length,
        // disk_number,
        // internal_file_attributes,
        external_file_attributes,
        offset,
        ..
    } = block;

    let is_utf8 = ZipFlags::matching(flags, ZipFlags::LanguageEncoding);

    let mut file_name_raw = read_variable_length_byte_field(reader, file_name_length as usize)?;
    let extra_fields_raw = read_variable_length_byte_field(reader, extra_field_length as usize)?;
    let file_comment_raw = read_variable_length_byte_field(reader, file_comment_length as usize)?;
    let file_comment: Box<str> = if is_utf8 {
        String::from_utf8_lossy(&file_comment_raw).into()
    } else {
        file_comment_raw.from_cp437()?.into()
    };

    let (version_made_by, system) = System::extract_bytes(version_made_by);
    let extra_fields = ExtraFields::parse(&extra_fields_raw, &block, false)?;
    // Construct the result
    let mut result = ZipFileData {
        system,
        version_made_by,
        compression_method: CompressionMethod::parse_from_u16(compression_method),
        last_modified_time: DateTime::try_from_msdos(last_mod_date, last_mod_time).ok(),
        crc32,
        compressed_size: compressed_size.into(),
        uncompressed_size: uncompressed_size.into(),
        flags: ZipFileFlags(flags),
        file_comment,
        header_start: offset.into(),
        central_header_start,
        data_start: OnceLock::new(),
        external_attributes: external_file_attributes,
        large_file: false,
        extra_fields,
    };
    result.apply_extra_fields(&mut file_name_raw)?;

    // Account for shifted zip offsets.
    result.header_start = result
        .header_start
        .checked_add(archive_offset)
        .ok_or(invalid!("Archive header is too large"))?;

    Ok((result, file_name_raw))
}

/// A trait for exposing file metadata inside the zip.
pub trait HasZipMetadata {
    /// Get the file metadata
    fn get_metadata(&self) -> &ZipFileData;
}

/// Options for reading a file from an archive.
#[derive(Default)]
#[non_exhaustive]
pub struct ZipReadOptions<'a> {
    /// The password to use when decrypting the file.  This is ignored if not required.
    pub(crate) password: Option<&'a [u8]>,
    /// Ignore the value of the encryption flag and proceed as if the file were plaintext.
    pub(crate) ignore_encryption_flag: bool,
    /// Ignore the crc32 of the file
    pub(crate) ignore_crc: bool,
    /// override the compressed_size for stream read
    force_compressed_size: Option<u64>,
    /// override the uncompressed_size for stream read
    force_uncompressed_size: Option<u64>,
    /// override the checksum for stream read
    force_crc: Option<u32>,
    /// The uncompressed size is not known (streamed entry with a data descriptor), so it
    /// can't bound the output.
    pub(crate) size_unknown: bool,
    /// Enforce consistency between the local file header and central directory header.
    pub(crate) enforce_header_consistency: bool,
}

impl<'a> ZipReadOptions<'a> {
    /// Create a new set of options with the default values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the password, if any, to use.  Return for chaining.
    #[must_use]
    pub fn password(mut self, password: Option<&'a [u8]>) -> Self {
        self.password = password;
        self
    }

    /// Set the ignore encryption flag.  Return for chaining.
    #[must_use]
    pub fn ignore_encryption_flag(mut self, ignore: bool) -> Self {
        self.ignore_encryption_flag = ignore;
        self
    }

    /// Ignore the CRC32 of the file
    #[must_use]
    pub fn ignore_crc32(mut self, should_ignore: bool) -> Self {
        self.ignore_crc = should_ignore;
        self
    }

    /// Override the compressed_size
    #[must_use]
    pub fn override_compressed_size(mut self, comp_size: u64) -> Self {
        self.force_compressed_size = Some(comp_size);
        self
    }

    /// Override the uncompressed_size
    #[must_use]
    pub fn override_uncompressed_size(mut self, uncomp_size: u64) -> Self {
        self.force_uncompressed_size = Some(uncomp_size);
        self
    }

    /// Override the checksum
    #[must_use]
    pub fn override_crc(mut self, crc: u32) -> Self {
        self.force_crc = Some(crc);
        self
    }

    /// Enforces that the local file header (LFH) matches the central directory header (CDFH).
    #[must_use]
    pub fn enforce_header_consistency(mut self, enforce: bool) -> Self {
        self.enforce_header_consistency = enforce;
        self
    }
}

/// Helper to extract Info-ZIP UnicodePath (0x7075) extra field content and CRC32.
pub(crate) fn get_upath(extra_fields: &ExtraFields) -> (Option<&[u8]>, Option<u32>) {
    extra_fields
        .inner
        .iter()
        .rev()
        .find_map(|f| match f {
            ExtraField::UnicodePath(u) => Some((&*u.content, u.crc32)),
            _ => None,
        })
        .unzip()
}

/// Data descriptor / payload metadata (CRC32 and sizes) that can be verified either
/// immediately or after reading a data descriptor.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub(crate) struct HeaderDataDescriptorMetadata {
    pub(crate) crc32: u32,
    pub(crate) compressed_size: u64,
    pub(crate) uncompressed_size: u64,
    pub(crate) is_pending_data_descriptor: bool,
}

impl HeaderDataDescriptorMetadata {
    /// Validates data descriptor fields between local and central headers.
    pub(crate) fn validate_matches(&self, central: &Self) -> ZipResult<()> {
        if self.crc32 != central.crc32 && (!self.is_pending_data_descriptor || self.crc32 != 0) {
            return Err(invalid!(
                "header inconsistency detected: CRC32 checksum mismatch (does not match central directory)"
            ));
        }

        if self.compressed_size != central.compressed_size
            && (!self.is_pending_data_descriptor || self.compressed_size != 0)
        {
            return Err(invalid!(
                "header inconsistency detected: compressed size mismatch (does not match central directory)"
            ));
        }

        if self.uncompressed_size != central.uncompressed_size
            && (!self.is_pending_data_descriptor || self.uncompressed_size != 0)
        {
            return Err(invalid!(
                "header inconsistency detected: uncompressed size mismatch (does not match central directory)"
            ));
        }

        Ok(())
    }
}

/// Unified header metadata used to validate consistency between Local and Central headers.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HeaderConsistencyMetadata<T: AsRef<[u8]>, U: AsRef<[u8]> = T> {
    pub(crate) name: T,
    pub(crate) upath: Option<U>,
    pub(crate) upath_crc32: Option<u32>,
    pub(crate) compression_method: u16,
    pub(crate) is_encrypted: bool,
    pub(crate) is_using_data_descriptor: bool,
    pub(crate) last_modified_time: Option<DateTime>,
    pub(crate) data: HeaderDataDescriptorMetadata,
    #[cfg(feature = "aes-crypto")]
    pub(crate) aes: Option<crate::extra_fields::AexEncryption>,
}

impl HeaderConsistencyMetadata<&[u8]> {
    pub(crate) fn into_owned(self) -> HeaderConsistencyMetadata<Box<[u8]>> {
        HeaderConsistencyMetadata {
            name: self.name.into(),
            upath: self.upath.map(Into::into),
            upath_crc32: self.upath_crc32,
            compression_method: self.compression_method,
            is_encrypted: self.is_encrypted,
            is_using_data_descriptor: self.is_using_data_descriptor,
            last_modified_time: self.last_modified_time,
            data: self.data,
            #[cfg(feature = "aes-crypto")]
            aes: self.aes,
        }
    }
}

impl<T: AsRef<[u8]>, U: AsRef<[u8]>> HeaderConsistencyMetadata<T, U> {
    /// Validates header identity (filename, UnicodePath, compression method, and flags).
    pub(crate) fn validate_identity_matches<T2: AsRef<[u8]>, U2: AsRef<[u8]>>(
        &self,
        central: &HeaderConsistencyMetadata<T2, U2>,
    ) -> ZipResult<()> {
        let local_effective = self
            .upath
            .as_ref()
            .map(AsRef::as_ref)
            .unwrap_or_else(|| self.name.as_ref());

        if self.upath.as_ref().map(AsRef::as_ref) != central.upath.as_ref().map(AsRef::as_ref)
            || self.upath_crc32 != central.upath_crc32
        {
            return Err(invalid!(
                "header inconsistency detected: UnicodePath (0x7075) extra field mismatch for '{}'",
                String::from_utf8_lossy(local_effective)
            ));
        }

        if self.name.as_ref() != central.name.as_ref() {
            return Err(invalid!(
                "header inconsistency detected: Local file header filename mismatch (does not match central directory)"
            ));
        }

        if self.compression_method != central.compression_method {
            return Err(invalid!(
                "header inconsistency detected: compression method mismatch (does not match central directory)"
            ));
        }

        if self.is_encrypted != central.is_encrypted {
            return Err(invalid!(
                "header inconsistency detected: encryption flag mismatch for '{}'",
                String::from_utf8_lossy(self.name.as_ref())
            ));
        }

        if self.is_using_data_descriptor != central.is_using_data_descriptor {
            return Err(invalid!(
                "header inconsistency detected: data descriptor flag mismatch for '{}'",
                String::from_utf8_lossy(self.name.as_ref())
            ));
        }

        if self.last_modified_time != central.last_modified_time {
            return Err(invalid!(
                "header inconsistency detected: last modified time mismatch for '{}'",
                String::from_utf8_lossy(self.name.as_ref())
            ));
        }

        #[cfg(feature = "aes-crypto")]
        if self.aes != central.aes {
            return Err(invalid!(
                "header inconsistency detected: AES encryption metadata mismatch for '{}'",
                String::from_utf8_lossy(self.name.as_ref())
            ));
        }

        Ok(())
    }

    /// Validates data descriptor and payload consistency.
    pub(crate) fn validate_data_matches<T2: AsRef<[u8]>, U2: AsRef<[u8]>>(
        &self,
        central: &HeaderConsistencyMetadata<T2, U2>,
    ) -> ZipResult<()> {
        self.data.validate_matches(&central.data)
    }

    /// Validates that local header metadata matches central directory metadata.
    pub(crate) fn validate_matches<T2: AsRef<[u8]>, U2: AsRef<[u8]>>(
        &self,
        central: &HeaderConsistencyMetadata<T2, U2>,
    ) -> ZipResult<()> {
        self.validate_identity_matches(central)?;
        self.validate_data_matches(central)?;
        Ok(())
    }
}
