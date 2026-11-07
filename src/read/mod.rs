//! Types for reading ZIP archives

use crate::compression::CompressionMethod;
use crate::cp437::FromCp437;
use crate::datetime::DateTime;
use crate::extra_fields::{ExtraField, ExtraFields};
use crate::format::blocks::{CentralDirectoryEndInfo, DataAndPosition, ZipCentralEntryBlock};
use crate::format::flags::{ZipFileFlags, ZipFlags};
use crate::format::system::System;
use crate::result::{ZipError, ZipResult, invalid};
use crate::types::{FilesByNameMap, ZipFileData};
use indexmap::IndexMap;
use std::io::{self, Read, Seek};
use std::sync::OnceLock;
use typed_path::{Utf8Component, Utf8WindowsComponent, Utf8WindowsPath};

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
pub(crate) mod zip_iterable;

pub(crate) mod zipfile;
pub use zipfile::{
    OwnedZipFileEntry, ZipFile, ZipFileEntry, ZipFileEntryWithDataDescriptor, ZipFileSeek,
};

pub(crate) mod zip_archive;
pub use zip_archive::{ZipArchive, ZipArchiveMetadata};

#[cfg(feature = "aes-crypto")]
pub use crate::aes::AesInfo;

#[derive(Debug)]
pub(crate) struct CentralDirectoryInfo {
    pub(crate) archive_offset: u64,
    pub(crate) directory_start: u64,
    pub(crate) directory_size: u64,
    pub(crate) number_of_files: usize,
    pub(crate) disk_number: u32,
    pub(crate) disk_with_central_directory: u32,
}

impl<'a> TryFrom<&'a CentralDirectoryEndInfo> for CentralDirectoryInfo {
    type Error = ZipError;

    fn try_from(value: &'a CentralDirectoryEndInfo) -> Result<Self, Self::Error> {
        let (
            relative_cd_offset,
            directory_size,
            number_of_files,
            disk_number,
            disk_with_central_directory,
        ) = match &value.eocd64 {
            Some(DataAndPosition { data: eocd64, .. }) => {
                if eocd64.number_of_files_on_this_disk > eocd64.number_of_files {
                    return Err(invalid!(
                        "ZIP64 footer indicates more files on this disk than in the whole archive"
                    ));
                }
                (
                    eocd64.central_directory_offset,
                    eocd64.central_directory_size,
                    eocd64.number_of_files as usize,
                    eocd64.disk_number,
                    eocd64.disk_with_central_directory,
                )
            }
            _ => (
                u64::from(value.eocd.data.central_directory_offset),
                u64::from(value.eocd.data.central_directory_size),
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
            directory_size,
            number_of_files,
            disk_number,
            disk_with_central_directory,
        })
    }
}

impl<R: Read + Seek> ZipArchive<R> {
    pub(crate) fn merge_contents<W: io::Write + Seek>(self, mut w: W) -> ZipResult<FilesByNameMap> {
        let Self { mut reader, shared } = self;
        if shared.files.is_empty() {
            return Ok(IndexMap::new());
        }

        /* The first file header will probably start at the beginning of the file, but zip doesn't
         * enforce that, and executable zips like PEX files will have a shebang line so will
         * definitely be greater than 0.
         *
         * assert_eq!(0, new_files[0].header_start); // Avoid this.
         */

        fn prepare_merge_files<I>(
            files: I,
            first_new_file_header_start: u64,
        ) -> ZipResult<FilesByNameMap>
        where
            I: Iterator<Item = (Box<[u8]>, ZipFileData)>,
        {
            let mut new_files = IndexMap::with_capacity(files.size_hint().0);
            for (name, file) in files {
                let mut file = file;
                file.header_start = file
                    .header_start
                    .checked_add(first_new_file_header_start)
                    .ok_or(invalid!(
                        "new header start from merge would have been too large"
                    ))?;
                file.central_header_start = 0;
                if let Some(old_data_start) = file.data_start.take() {
                    let new_data_start = old_data_start
                        .checked_add(first_new_file_header_start)
                        .ok_or(invalid!(
                            "new data start from merge would have been too large"
                        ))?;
                    file.data_start.get_or_init(|| new_data_start);
                }
                new_files.insert(name, file);
            }
            Ok(new_files)
        }

        let (new_files, dir_start) = match std::sync::Arc::try_unwrap(shared) {
            Ok(metadata) => (
                prepare_merge_files(metadata.files.into_iter(), w.stream_position()?)?,
                metadata.dir_start,
            ),
            Err(shared) => (
                prepare_merge_files(
                    shared
                        .files
                        .iter()
                        .map(|(name, file)| (name.clone(), file.clone())),
                    w.stream_position()?,
                )?,
                shared.dir_start,
            ),
        };

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
        reader.rewind()?;
        /* Find the end of the file data. */
        let length_to_read = dir_start;
        /* Copy over file data from source archive directly. */
        io::copy(&mut reader.take(length_to_read), &mut w)?;

        /* Return the files we've just written to the data stream. */
        Ok(new_files)
    }
}

#[inline]
fn read_variable_length_byte_field<R: Read>(reader: &mut R, len: usize) -> ZipResult<Vec<u8>> {
    if len == 0 {
        return Ok(Vec::new());
    }
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
    let file_comment: Box<str> = if file_comment_length == 0 {
        Box::default()
    } else {
        let file_comment_raw =
            read_variable_length_byte_field(reader, file_comment_length as usize)?;
        if is_utf8 {
            String::from_utf8_lossy(&file_comment_raw).into()
        } else {
            file_comment_raw.from_cp437()?.into()
        }
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
        overridden_header_name: None,
        header_language_encoding: ZipFlags::matching(flags, ZipFlags::LanguageEncoding),
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
#[derive(Clone, Default, Debug)]
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
    ///
    /// AES HMAC authentication is only performed once the end of the file is read. If any read
    /// method returns an error, the file may have been tampered with. `ZipCrypto` provides no
    /// authentication at all.
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
pub(crate) struct HeaderConsistencyMetadata<T: AsRef<[u8]>> {
    pub(crate) name: T,
    pub(crate) overridden_header_name: Option<T>,
    pub(crate) is_directory: bool,
    pub(crate) has_language_encoding: bool,
    pub(crate) upath: Option<T>,
    pub(crate) upath_crc32: Option<u32>,
    pub(crate) compression_method: u16,
    pub(crate) is_encrypted: bool,
    pub(crate) is_using_data_descriptor: bool,
    /// Flag bits the decompressor reads (bits 1-2 for Implode), zero for other methods.
    pub(crate) decoder_flags: u16,
    pub(crate) last_modified_time: Option<DateTime>,
    pub(crate) data: HeaderDataDescriptorMetadata,
    #[cfg(feature = "aes-crypto")]
    pub(crate) aes: Option<crate::extra_fields::AexEncryption>,
}

impl HeaderConsistencyMetadata<&[u8]> {
    pub(crate) fn into_owned(self) -> HeaderConsistencyMetadata<Box<[u8]>> {
        HeaderConsistencyMetadata {
            name: self.name.into(),
            overridden_header_name: self.overridden_header_name.map(Into::into),
            is_directory: self.is_directory,
            has_language_encoding: self.has_language_encoding,
            upath: self.upath.map(Into::into),
            upath_crc32: self.upath_crc32,
            compression_method: self.compression_method,
            is_encrypted: self.is_encrypted,
            is_using_data_descriptor: self.is_using_data_descriptor,
            decoder_flags: self.decoder_flags,
            last_modified_time: self.last_modified_time,
            data: self.data,
            #[cfg(feature = "aes-crypto")]
            aes: self.aes,
        }
    }
}

impl<T: AsRef<[u8]>> HeaderConsistencyMetadata<T> {
    /// Validates header identity (filename, UnicodePath, compression method, and flags).
    pub(crate) fn validate_identity_matches<T2: AsRef<[u8]>>(
        &self,
        central: &HeaderConsistencyMetadata<T2>,
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

        validate_language_encoding_upath(
            self.has_language_encoding,
            self.name.as_ref(),
            self.overridden_header_name.as_ref().map(AsRef::as_ref),
            self.upath.as_ref().map(AsRef::as_ref),
        )?;
        validate_language_encoding_upath(
            central.has_language_encoding,
            central.name.as_ref(),
            central.overridden_header_name.as_ref().map(AsRef::as_ref),
            central.upath.as_ref().map(AsRef::as_ref),
        )?;

        if let (Some(upath), Some(overridden_header_name)) =
            (self.upath.as_ref(), self.overridden_header_name.as_ref())
        {
            validate_unicode_path_name(
                overridden_header_name.as_ref(),
                upath.as_ref(),
                self.is_directory && central.is_directory,
            )?;
        }
        if let (Some(upath), Some(overridden_header_name)) = (
            central.upath.as_ref(),
            central.overridden_header_name.as_ref(),
        ) {
            validate_unicode_path_name(
                overridden_header_name.as_ref(),
                upath.as_ref(),
                self.is_directory && central.is_directory,
            )?;
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

        if self.decoder_flags != central.decoder_flags {
            return Err(invalid!(
                "header inconsistency detected: compression flags mismatch for '{}'",
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
    pub(crate) fn validate_data_matches<T2: AsRef<[u8]>>(
        &self,
        central: &HeaderConsistencyMetadata<T2>,
    ) -> ZipResult<()> {
        self.data.validate_matches(&central.data)
    }

    /// Validates that local header metadata matches central directory metadata.
    pub(crate) fn validate_matches<T2: AsRef<[u8]>>(
        &self,
        central: &HeaderConsistencyMetadata<T2>,
    ) -> ZipResult<()> {
        self.validate_identity_matches(central)?;
        self.validate_data_matches(central)?;
        Ok(())
    }
}

fn validate_language_encoding_upath(
    has_language_encoding: bool,
    file_name: &[u8],
    overridden_header_name: Option<&[u8]>,
    upath: Option<&[u8]>,
) -> ZipResult<()> {
    if has_language_encoding
        && let Some(upath) = upath
        && overridden_header_name.unwrap_or(file_name) != upath
    {
        return Err(invalid!(
            "header inconsistency detected: UnicodePath (0x7075) does not match UTF-8 file name"
        ));
    }
    Ok(())
}

fn validate_unicode_path_name(
    main_name: &[u8],
    unicode_path: &[u8],
    is_directory: bool,
) -> ZipResult<()> {
    // 1. Null-byte poisoning check
    if main_name.contains(&0) || unicode_path.contains(&0) {
        return Err(invalid!(
            "header inconsistency detected: filename contains null byte"
        ));
    }

    // 2. Directory trailing separator parity
    let main_has_trailing = main_name.ends_with(b"/") || main_name.ends_with(b"\\");
    let unicode_has_trailing = unicode_path.ends_with(b"/") || unicode_path.ends_with(b"\\");
    if main_has_trailing != unicode_has_trailing {
        return Err(invalid!(
            "header inconsistency detected: UnicodePath (0x7075) directory status differs from file name"
        ));
    }

    let main_name_str = String::from_utf8_lossy(main_name);
    let unicode_path_str = std::str::from_utf8(unicode_path).map_err(|_| {
        invalid!("header inconsistency detected: UnicodePath (0x7075) is not valid UTF-8")
    })?;

    // 3. Prohibit dangerous Unicode control characters & homoglyph delimiters:
    // - Bidirectional overrides: U+202A..=U+202E, U+2066..=U+2069
    // - Fullwidth dots and slashes: U+FF0E, U+FF0F, U+FF3C, U+2044, U+2215
    // - ASCII control characters (< 0x20)
    for ch in unicode_path_str.chars() {
        if matches!(
            ch,
            '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{FF0E}'
                | '\u{FF0F}'
                | '\u{FF3C}'
                | '\u{2044}'
                | '\u{2215}'
                | '\0'..='\x1F'
                | '\x7F'
        ) {
            return Err(invalid!(
                "header inconsistency detected: UnicodePath (0x7075) contains illegal control or homoglyph character"
            ));
        }
    }

    let main_components: Vec<_> = Utf8WindowsPath::new(&main_name_str).components().collect();
    let unicode_components: Vec<_> = Utf8WindowsPath::new(unicode_path_str)
        .components()
        .collect();

    if main_components.len() != unicode_components.len() {
        return Err(invalid!(
            "header inconsistency detected: UnicodePath (0x7075) changes file name path components"
        ));
    }

    for (main, unicode) in main_components.iter().zip(&unicode_components) {
        if core::mem::discriminant(main) != core::mem::discriminant(unicode) {
            return Err(invalid!(
                "header inconsistency detected: UnicodePath (0x7075) changes file name path components"
            ));
        }

        let (main_str, unicode_str) = (main.as_str(), unicode.as_str());

        // 4. Windows trailing dot/space spoofing check
        let main_trimmed = main_str.trim_end_matches(['.', ' ']);
        let unicode_trimmed = unicode_str.trim_end_matches(['.', ' ']);
        if (main_trimmed != main_str) != (unicode_trimmed != unicode_str) {
            return Err(invalid!(
                "header inconsistency detected: UnicodePath (0x7075) has divergent trailing dots or spaces"
            ));
        }

        // Exception: DOS 8.3 alias compatibility (e.g. LONGFI~1.TXT for LongFileName.txt).
        // Tools in the 1990s wrote 8.3 aliases in the main header and long names in 0x7075.
        if let Some(prefix) = parse_dos_8_3_alias(main_trimmed) {
            let unicode_stem = match unicode_trimmed.rsplit_once('.') {
                Some((stem, _)) => stem,
                None => unicode_trimmed,
            };
            if is_ascii_subsequence(prefix, unicode_stem) {
                continue;
            }
        }

        if main_str.is_ascii() && unicode_str.is_ascii() {
            // Case-insensitive ASCII comparison: handles uppercase fallback tools (e.g. README.TXT vs readme.txt).
            if !main_str.eq_ignore_ascii_case(unicode_str) {
                return Err(invalid!(
                    "header inconsistency detected: UnicodePath (0x7075) changes file name path components"
                ));
            }
        } else {
            // When either component has non-ASCII characters, the raw main_name must represent a valid
            // fallback (transliteration / lossy representation). ASCII characters in either component
            // must appear in the other in relative order (subsequence match).
            let matches = is_ascii_subsequence(unicode_str, main_str)
                || is_ascii_subsequence(main_str, unicode_str);

            if !matches {
                return Err(invalid!(
                    "header inconsistency detected: UnicodePath (0x7075) changes file name path components"
                ));
            }
        }
    }

    if !is_directory {
        let unicode_extension = file_extension(unicode_path_str);
        let main_extension = file_extension(&main_name_str);
        let extensions_consistent = match (main_extension, unicode_extension) {
            (None, None) => true,
            (Some(main_ext), Some(unicode_ext)) => {
                if main_ext.eq_ignore_ascii_case(unicode_ext) {
                    true
                } else {
                    // Legitimate non-ASCII extension with ASCII fallback (e.g. Cyrillic/Greek transliteration)
                    main_ext.is_ascii() && !unicode_ext.is_ascii()
                }
            }
            _ => false,
        };
        if !extensions_consistent {
            return Err(invalid!(
                "header inconsistency detected: UnicodePath (0x7075) file extension differs from file name"
            ));
        }
    }

    Ok(())
}

fn to_subsequence_ascii(c: char) -> Option<char> {
    match c {
        // Turkish dotted capital İ (U+0130) and dotless lowercase ı (U+0131)
        '\u{0130}' | '\u{0131}' => Some('i'),
        c if c.is_ascii() => Some(c.to_ascii_lowercase()),
        _ => None,
    }
}

fn is_ascii_subsequence(sub: &str, full: &str) -> bool {
    let mut sub_iter = sub.chars().filter_map(to_subsequence_ascii);

    let mut target = match sub_iter.next() {
        Some(c) => c,
        None => return true,
    };

    for ch in full.chars().filter_map(to_subsequence_ascii) {
        if ch == target {
            match sub_iter.next() {
                Some(next_c) => target = next_c,
                None => return true,
            }
        }
    }
    false
}

fn parse_dos_8_3_alias(component: &str) -> Option<&str> {
    let stem = match component.rsplit_once('.') {
        Some((s, _)) => s,
        None => component,
    };
    if stem.len() > 8 || !stem.contains('~') {
        return None;
    }
    let tilde_idx = stem.rfind('~')?;
    let (prefix, suffix) = stem.split_at(tilde_idx);
    let digits = &suffix[1..];
    if !prefix.is_empty() && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        Some(prefix)
    } else {
        None
    }
}

fn file_extension(path: &str) -> Option<&str> {
    let Utf8WindowsComponent::Normal(file_name) =
        Utf8WindowsPath::new(path).components().next_back()?
    else {
        return None;
    };
    let dot_idx = file_name.rfind('.')?;
    let stem = &file_name[..dot_idx];
    let ext = &file_name[dot_idx + 1..];
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    Some(ext)
}

#[test]
fn test_multiple_file_extension() {
    assert_eq!(file_extension("archive.tar.gz"), Some("gz"));
    assert_eq!(file_extension("archive.résumé.gz"), Some("gz"));
    assert_eq!(file_extension("file.txt"), Some("txt"));
    assert_eq!(file_extension(".gitignore"), None);
    assert_eq!(file_extension("no_extension"), None);
}

#[test]
fn test_validate_unicode_path_name_compatibility() {
    // 1. German ß / ss & CP437
    assert!(validate_unicode_path_name(b"strasse.txt", "straße.txt".as_bytes(), false).is_ok());
    assert!(validate_unicode_path_name(b"STRASSE.TXT", "straße.txt".as_bytes(), false).is_ok());
    assert!(validate_unicode_path_name(b"Stra\xe1e.txt", "straße.txt".as_bytes(), false).is_ok());
    assert!(validate_unicode_path_name(b"strae.txt", "straße.txt".as_bytes(), false).is_ok());
    assert!(validate_unicode_path_name(b"stra_e.txt", "straße.txt".as_bytes(), false).is_ok());
    assert!(validate_unicode_path_name(b"stra?e.txt", "straße.txt".as_bytes(), false).is_ok());
    assert!(validate_unicode_path_name(b"M\x81ller.txt", "Müller.txt".as_bytes(), false).is_ok());
    assert!(validate_unicode_path_name(b"caf\x82.txt", "café.txt".as_bytes(), false).is_ok());

    // 2. Turkish I / İ / ı (including Windows-1254 encodings)
    assert!(validate_unicode_path_name(b"ISTANBUL.TXT", "İstanbul.txt".as_bytes(), false).is_ok());
    assert!(validate_unicode_path_name(b"istanbul.txt", "İstanbul.txt".as_bytes(), false).is_ok());
    assert!(
        validate_unicode_path_name(b"\xddSTANBUL.TXT", "İstanbul.txt".as_bytes(), false).is_ok()
    );
    assert!(
        validate_unicode_path_name(b"DIYARBAKIR.TXT", "Diyarbakır.txt".as_bytes(), false).is_ok()
    );
    assert!(
        validate_unicode_path_name(b"D\xfdYARBAKIR.TXT", "Diyarbakır.txt".as_bytes(), false)
            .is_ok()
    );
    assert!(
        validate_unicode_path_name(b"diyarbak\xfdr.txt", "Diyarbakır.txt".as_bytes(), false)
            .is_ok()
    );
    assert!(validate_unicode_path_name(b"ATATURK.TXT", "Atatürk.txt".as_bytes(), false).is_ok());
    assert!(validate_unicode_path_name(b"ataturk.txt", "Atatürk.txt".as_bytes(), false).is_ok());
    assert!(validate_unicode_path_name(b"TURKIYE.TXT", "Türkiye.txt".as_bytes(), false).is_ok());
    assert!(
        validate_unicode_path_name(b"CANAKKALE.TXT", "Çanakkale.txt".as_bytes(), false).is_ok()
    );

    // 3. DOS & OS/2 8.3 Aliases and Fallbacks
    assert!(
        validate_unicode_path_name(b"LONGFI~1.TXT", "LongFileName.txt".as_bytes(), false).is_ok()
    );
    assert!(
        validate_unicode_path_name(
            b"DOCUME~2.PDF",
            "DocumentSpecialReport.pdf".as_bytes(),
            false
        )
        .is_ok()
    );
    assert!(
        validate_unicode_path_name(b"ISTANB~1.DOC", "İstanbulReport.doc".as_bytes(), false).is_ok()
    );
    // DOS 8.3 aliases with prefix matching:
    assert!(
        validate_unicode_path_name(
            b"VERYLO~1.TXT",
            "VeryLongDocumentName.txt".as_bytes(),
            false
        )
        .is_ok()
    );
    // Adversarial: arbitrary 8.3 ASCII names cannot cloak divergent Unicode paths:
    assert!(
        validate_unicode_path_name(b"MYDOC.TXT", "VeryLongDocumentName.txt".as_bytes(), false)
            .is_err()
    );
    assert!(validate_unicode_path_name(b"readme.txt", "malwaré.txt".as_bytes(), false).is_err());
    // Adversarial: both 8.3 ASCII but divergent names must be blocked:
    assert!(validate_unicode_path_name(b"safe.txt", "evil.txt".as_bytes(), false).is_err());
    assert!(validate_unicode_path_name(b"SAFE.TXT", "safe.txt".as_bytes(), false).is_ok());
    assert!(
        validate_unicode_path_name(b"LONGFI~1.EXE", "LongFileName.pdf".as_bytes(), false).is_err()
    );

    // 4. Non-ASCII Extensions with ASCII Fallback
    assert!(validate_unicode_path_name(b"document.doc", "document.дос".as_bytes(), false).is_ok());
    assert!(validate_unicode_path_name(b"report.pdf", "report.рdf".as_bytes(), false).is_ok());
    assert!(validate_unicode_path_name(b"document.txt", "document.exe".as_bytes(), false).is_err());

    // 5. Latin and Non-Latin alphabets / Hungarian diacritics & dropped characters
    assert!(validate_unicode_path_name(b"arxeio.txt", "αρχείο.txt".as_bytes(), false).is_ok());
    assert!(validate_unicode_path_name(b"document.txt", "документ.txt".as_bytes(), false).is_ok());
    assert!(validate_unicode_path_name(b"dok.txt", "документ.txt".as_bytes(), false).is_ok());
    assert!(
        validate_unicode_path_name(b"arvizturo.txt", "árvíztűrő.txt".as_bytes(), false).is_ok()
    );
    assert!(validate_unicode_path_name(b"ervnyes.txt", "érvényes.txt".as_bytes(), false).is_ok());
    assert!(validate_unicode_path_name(b"rvztr.txt", "árvíztűrő.txt".as_bytes(), false).is_ok());
    assert!(
        validate_unicode_path_name(b"?rv?zt?r?.txt", "árvíztűrő.txt".as_bytes(), false).is_ok()
    );
    assert!(
        validate_unicode_path_name(b"\xA0rv\xA1zturo.txt", "árvíztűrő.txt".as_bytes(), false)
            .is_ok()
    );
    assert!(
        validate_unicode_path_name(b"\xA0rv\xA1ztr.txt", "árvíztűrő.txt".as_bytes(), false).is_ok()
    );
    assert!(
        validate_unicode_path_name(b"\xA0rv\xA1zt?r?.txt", "árvíztűrő.txt".as_bytes(), false)
            .is_ok()
    );
    assert!(
        validate_unicode_path_name(
            b"\xA0rv\xA1zt\x81r\x94.txt",
            "árvíztűrő.txt".as_bytes(),
            false
        )
        .is_ok()
    );
}
