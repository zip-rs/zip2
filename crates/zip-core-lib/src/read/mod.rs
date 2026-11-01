//! Types for reading ZIP archives

use crate::datetime::DateTime;
use crate::extra_fields::{ExtraField, ExtraFields};
use crate::result::{ZipResult, invalid};
use crate::types::ZipFileData;

pub mod extract;
pub use extract::{ExtractOptions, RootDirFilter, SymlinkExtractAction, root_dir_common_filter};

#[cfg(feature = "aes-crypto")]
pub use crate::aes::AesInfo;

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
    pub password: Option<&'a [u8]>,
    /// Ignore the value of the encryption flag and proceed as if the file were plaintext.
    pub ignore_encryption_flag: bool,
    /// Ignore the crc32 of the file
    pub ignore_crc: bool,
    /// override the compressed_size for stream read
    force_compressed_size: Option<u64>,
    /// override the uncompressed_size for stream read
    force_uncompressed_size: Option<u64>,
    /// override the checksum for stream read
    force_crc: Option<u32>,
    /// The uncompressed size is not known (streamed entry with a data descriptor), so it
    /// can't bound the output.
    pub size_unknown: bool,
    /// Enforce consistency between the local file header and central directory header.
    pub enforce_header_consistency: bool,
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

    #[doc(hidden)]
    pub fn forced_compressed_size(&self) -> Option<u64> {
        self.force_compressed_size
    }

    /// Override the uncompressed_size
    #[must_use]
    pub fn override_uncompressed_size(mut self, uncomp_size: u64) -> Self {
        self.force_uncompressed_size = Some(uncomp_size);
        self
    }

    #[doc(hidden)]
    pub fn forced_uncompressed_size(&self) -> Option<u64> {
        self.force_uncompressed_size
    }

    /// Override the checksum
    #[must_use]
    pub fn override_crc(mut self, crc: u32) -> Self {
        self.force_crc = Some(crc);
        self
    }

    #[doc(hidden)]
    pub fn forced_crc(&self) -> Option<u32> {
        self.force_crc
    }

    /// Enforces that the local file header (LFH) matches the central directory header (CDFH).
    #[must_use]
    pub fn enforce_header_consistency(mut self, enforce: bool) -> Self {
        self.enforce_header_consistency = enforce;
        self
    }
}

/// Helper to extract Info-ZIP UnicodePath (0x7075) extra field content and CRC32.
pub fn get_upath(extra_fields: &ExtraFields) -> (Option<&[u8]>, Option<u32>) {
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
pub struct HeaderDataDescriptorMetadata {
    pub crc32: u32,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub is_pending_data_descriptor: bool,
}

impl HeaderDataDescriptorMetadata {
    /// Validates data descriptor fields between local and central headers.
    pub fn validate_matches(&self, central: &Self) -> ZipResult<()> {
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
pub struct HeaderConsistencyMetadata<T: AsRef<[u8]>, U: AsRef<[u8]> = T> {
    pub name: T,
    pub upath: Option<U>,
    pub upath_crc32: Option<u32>,
    pub compression_method: u16,
    pub is_encrypted: bool,
    pub is_using_data_descriptor: bool,
    /// Flag bits the decompressor reads (bits 1-2 for Implode), zero for other methods.
    pub decoder_flags: u16,
    pub last_modified_time: Option<DateTime>,
    pub data: HeaderDataDescriptorMetadata,
    #[cfg(feature = "aes-crypto")]
    pub aes: Option<crate::extra_fields::AexEncryption>,
}

impl HeaderConsistencyMetadata<&[u8]> {
    pub fn into_owned(self) -> HeaderConsistencyMetadata<Box<[u8]>> {
        HeaderConsistencyMetadata {
            name: self.name.into(),
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

impl<T: AsRef<[u8]>, U: AsRef<[u8]>> HeaderConsistencyMetadata<T, U> {
    /// Validates header identity (filename, UnicodePath, compression method, and flags).
    pub fn validate_identity_matches<T2: AsRef<[u8]>, U2: AsRef<[u8]>>(
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
    pub fn validate_data_matches<T2: AsRef<[u8]>, U2: AsRef<[u8]>>(
        &self,
        central: &HeaderConsistencyMetadata<T2, U2>,
    ) -> ZipResult<()> {
        self.data.validate_matches(&central.data)
    }

    /// Validates that local header metadata matches central directory metadata.
    pub fn validate_matches<T2: AsRef<[u8]>, U2: AsRef<[u8]>>(
        &self,
        central: &HeaderConsistencyMetadata<T2, U2>,
    ) -> ZipResult<()> {
        self.validate_identity_matches(central)?;
        self.validate_data_matches(central)?;
        Ok(())
    }
}
