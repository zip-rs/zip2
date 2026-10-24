//! Write Options

use crate::extra_fields::ExtendedTimestamp;
use core::fmt::Debug;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::CompressionMethod;
use crate::datetime::DateTime;
use crate::extra_fields::CustomExtraField;
use crate::format::ffi;
use crate::format::system::System;
use crate::result::{ZipResult, invalid};
use crate::zipcrypto::ZipCryptoKeys;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum EncryptWith<'k> {
    #[cfg(feature = "aes-crypto")]
    Aes {
        mode: crate::format::aes::AesMode,
        vendor_version: crate::format::aes::AesVendorVersion,
        // When the password is None, it means that we are reusing the previous encryption
        password: Option<&'k [u8]>,
        salt: Option<crate::aes::AesSalt>,
    },
    ZipCrypto(ZipCryptoKeys, PhantomData<&'k ()>),
}

#[cfg(feature = "_arbitrary")]
impl<'a> arbitrary::Arbitrary<'a> for EncryptWith<'a> {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        #[cfg(feature = "aes-crypto")]
        if bool::arbitrary(u)? {
            return Ok(EncryptWith::Aes {
                mode: crate::format::aes::AesMode::arbitrary(u)?,
                password: Some(u.arbitrary::<&[u8]>()?),
                vendor_version: crate::format::aes::AesVendorVersion::Ae2,
                salt: None, // We don't need to test with random salt. It's only for testing or reproducible zips
            });
        }

        Ok(EncryptWith::ZipCrypto(
            ZipCryptoKeys::arbitrary(u)?,
            PhantomData,
        ))
    }
}

/// Metadata for a file to be written
#[non_exhaustive]
#[derive(Clone, Debug, Copy, Eq, PartialEq)]
pub struct FileOptions<'k, 'n, T: sealed::FileOptionExtension> {
    pub(crate) compression_method: CompressionMethod,
    pub(crate) compression_level: Option<i64>,
    pub(crate) external_attributes: Option<u32>,
    pub(crate) last_modified_time: DateTime,
    pub(crate) permissions: Option<u32>,
    pub(crate) large_file: bool,
    pub(crate) encrypt_with: Option<EncryptWith<'k>>,
    pub(crate) extended_options: T,
    pub(crate) alignment: u16,
    #[cfg(feature = "deflate-zopfli")]
    pub(super) zopfli_buffer_size: Option<usize>,
    pub(crate) system: Option<System>,
    pub(crate) name: Option<&'n [u8]>,
}
/// Simple File Options. Can be copied and good for simple writing zip files
pub type SimpleFileOptions = FileOptions<'static, 'static, ()>;

impl FileOptions<'static, 'static, ()> {
    pub(crate) const DEFAULT_FILE_PERMISSION: u32 = 0o644 | ffi::S_IFREG;
    pub(crate) const DEFAULT_DIR_PERMISSIONS: u32 = 0o755 | ffi::S_IFDIR; // rwxr-xr-x default for directories
    pub(crate) const DEFAULT_SYMLINK_PERMISSIONS: u32 = 0o777 | ffi::S_IFLNK;
}

pub(crate) mod sealed {
    use super::ExtendedFileOptions;
    use crate::extra_fields::CustomExtraField;
    use std::sync::Arc;

    pub trait Sealed {}
    /// File options Extensions
    #[doc(hidden)]
    pub trait FileOptionExtension: Default + Sealed {
        /// Extra Data
        fn extra_fields(&self) -> Option<&Arc<Vec<CustomExtraField>>>;
        /// File Comment
        fn file_comment(&self) -> Option<&str>;
        /// Take File Comment (moves ownership)
        fn take_file_comment(&mut self) -> Option<Box<str>>;
        /// Whether to check the extra fields with the reader's parser before writing
        fn validate_extra_fields(&self) -> bool {
            false
        }
    }
    impl Sealed for () {}
    impl FileOptionExtension for () {
        fn extra_fields(&self) -> Option<&Arc<Vec<CustomExtraField>>> {
            None
        }
        fn file_comment(&self) -> Option<&str> {
            None
        }
        fn take_file_comment(&mut self) -> Option<Box<str>> {
            None
        }
    }
    impl Sealed for ExtendedFileOptions {}

    impl FileOptionExtension for ExtendedFileOptions {
        fn extra_fields(&self) -> Option<&Arc<Vec<CustomExtraField>>> {
            Some(&self.extra_fields)
        }
        fn file_comment(&self) -> Option<&str> {
            self.file_comment.as_ref().map(Box::as_ref)
        }
        fn take_file_comment(&mut self) -> Option<Box<str>> {
            self.file_comment.take()
        }
        fn validate_extra_fields(&self) -> bool {
            self.validate_extra_fields
        }
    }
}

/// Adds Extra Data and Central Extra Data. It does not implement copy.
pub type FullFileOptions<'k, 'n> = FileOptions<'k, 'n, ExtendedFileOptions>;
/// The Extension for Extra Data and Central Extra Data
#[cfg_attr(feature = "_arbitrary", derive(arbitrary::Arbitrary))]
#[derive(Clone, Default, Eq, PartialEq)]
pub struct ExtendedFileOptions {
    pub(crate) extra_fields: Arc<Vec<CustomExtraField>>,
    pub(crate) file_comment: Option<Box<str>>,
    pub(crate) validate_extra_fields: bool,
}

impl ExtendedFileOptions {
    /// Adds an extra data field, unless we detect that it's invalid.
    ///
    /// # Parameters
    ///
    /// * `header_id` – The 2‑byte identifier of the ZIP extra field to add.
    ///   This value determines the type/format of `data` and should either be
    ///   one of the standard ZIP extra field IDs defined by the ZIP
    ///   specification or an application‑specific (vendor) ID.
    /// * `data` – The raw payload for the extra field, without the leading
    ///   header ID or length; those are derived from `header_id` and
    ///   `data.len()` and written automatically.
    /// * `central_only` – Controls where the extra field is stored:
    ///   * When `true`, the field is appended only to the central directory
    ///     extra data (`central_extra_data`), and the corresponding local file
    ///     header is left unchanged.
    ///   * When `false`, the field is appended to the local file header extra
    ///     data (`extra_data`) and may also be reflected in the central
    ///     directory, depending on how the ZIP is written.
    ///
    /// The combined size of all extra data (local + central) must not exceed
    /// `u16::MAX`. If adding this field would exceed that limit or produce an
    /// invalid extra data structure, an error is returned and no data is
    /// added.
    #[deprecated = "use add_extra_field()"]
    pub fn add_extra_data<D: AsRef<[u8]>>(
        &mut self,
        header_id: u16,
        data: D,
        central_only: bool,
    ) -> ZipResult<()> {
        self.add_extra_field(header_id, data, central_only)
    }
    /// Whether the reader's own extra-field parser accepts this field in every header it will be
    /// written to (local and central, or central only).
    pub(crate) fn extra_field_parses(header_id: u16, data: &[u8], central_only: bool) -> bool {
        use crate::extra_fields::{ExtraField, UsedExtraField};
        use crate::format::blocks::{Pod, ZipLocalEntryBlock};
        use std::io::Cursor;

        let Ok(size) = u16::try_from(data.len()) else {
            return false;
        };
        let mut serialized = Vec::with_capacity(data.len() + 4);
        serialized.extend_from_slice(&header_id.to_le_bytes());
        serialized.extend_from_slice(&size.to_le_bytes());
        serialized.extend_from_slice(data);
        let block = ZipLocalEntryBlock::zeroed();
        if ExtraField::parse(&mut Cursor::new(&serialized), &block, false).is_err() {
            return false;
        }
        // Only extended timestamps interpret their payload differently in local and central headers.
        central_only
            || header_id != UsedExtraField::ExtendedTimestamp.as_u16()
            || ExtendedTimestamp::try_from_reader(&mut Cursor::new(data), size, true).is_ok()
    }

    /// Adds an extra field, unless we detect that it's invalid.
    ///
    /// # Parameters
    ///
    /// * `header_id` – The 2‑byte identifier of the ZIP extra field to add.
    ///   This value determines the type/format of `data` and should either be
    ///   one of the standard ZIP extra field IDs defined by the ZIP
    ///   specification or an application‑specific (vendor) ID.
    /// * `data` – The raw payload for the extra field, without the leading
    ///   header ID or length; those are derived from `header_id` and
    ///   `data.len()` and written automatically.
    /// * `central_only` – Controls where the extra field is stored:
    ///   * When `true`, the field is appended only to the central directory
    ///     extra data (`central_extra_data`), and the corresponding local file
    ///     header is left unchanged.
    ///   * When `false`, the field is appended to the local file header extra
    ///     data (`extra_data`) and may also be reflected in the central
    ///     directory, depending on how the ZIP is written.
    ///
    /// The combined size of all extra data (local + central) must not exceed
    /// `u16::MAX`; if adding this field would exceed that limit, an error is
    /// returned and no data is added.
    ///
    /// By default `data` is otherwise stored verbatim, even for a header ID
    /// that this crate's reader interprets (such as `0x7075`, Info-ZIP Unicode
    /// Path) and would reject when the archive is read back. Opt in to
    /// checking it with [`FileOptions::validate_extra_fields`].
    pub fn add_extra_field<D: AsRef<[u8]>>(
        &mut self,
        header_id: u16,
        data: D,
        central_only: bool,
    ) -> ZipResult<()> {
        let data = data.as_ref();
        let len = data.len() + 4;
        let extra_fields_len: usize = self.extra_fields.iter().map(|x| x.full_size(false)).sum();
        if extra_fields_len + len > u16::MAX as usize {
            Err(invalid!("Extra data field would be longer than allowed"))
        } else if self.validate_extra_fields
            && !Self::extra_field_parses(header_id, data, central_only)
        {
            // Otherwise the archive is written, but `ZipArchive` (and `ZipWriter::new_append`)
            // reject it, e.g. an Info-ZIP Unicode Path field shorter than its 5-byte header.
            Err(invalid!(
                "Extra field {header_id:#06x} would not be readable: malformed data"
            ))
        } else {
            Arc::make_mut(&mut self.extra_fields).push(CustomExtraField::new(
                central_only,
                header_id,
                data.to_vec().into_boxed_slice(),
            ));
            Ok(())
        }
    }
}

impl Debug for ExtendedFileOptions {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> Result<(), core::fmt::Error> {
        f.debug_struct("ExtendedFileOptions")
            .field("extra_fields", &self.extra_fields)
            .field("file_comment", &self.file_comment)
            .field("validate_extra_fields", &self.validate_extra_fields)
            .finish()
    }
}

#[cfg(feature = "_arbitrary")]
impl<'k, 'n, 'a: 'k + 'n> arbitrary::Arbitrary<'a> for FileOptions<'k, 'n, ExtendedFileOptions> {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        use crate::extra_fields::EXTRA_FIELD_MAPPING;
        use crate::extra_fields::UsedExtraField;

        let mut options = FullFileOptions {
            compression_method: CompressionMethod::arbitrary(u)?,
            compression_level: if bool::arbitrary(u)? {
                Some(u.int_in_range(0..=24)?)
            } else {
                None
            },
            last_modified_time: DateTime::arbitrary(u)?,
            permissions: Option::<u32>::arbitrary(u)?,
            large_file: bool::arbitrary(u)?,
            encrypt_with: Option::<EncryptWith<'_>>::arbitrary(u)?,
            alignment: u16::arbitrary(u)?,
            #[cfg(feature = "deflate-zopfli")]
            zopfli_buffer_size: None,
            ..Default::default()
        };
        #[cfg(feature = "deflate-zopfli")]
        if options.compression_method == CompressionMethod::Deflated && bool::arbitrary(u)? {
            options.zopfli_buffer_size =
                Some(if bool::arbitrary(u)? { 2 } else { 3 } << u.int_in_range(8..=20)?);
        }
        u.arbitrary_loop(Some(0), Some(10), |u| {
            let header_id = u.int_in_range(2..=u16::MAX)?;
            if UsedExtraField::try_from(header_id).is_err()
                && EXTRA_FIELD_MAPPING.contains(&header_id)
            {
                return Err(arbitrary::Error::IncorrectFormat);
            }
            let data = Box::<[u8]>::arbitrary(u)?;
            options
                .add_extra_field(header_id, data, bool::arbitrary(u)?)
                .map_err(|_| arbitrary::Error::IncorrectFormat)?;
            Ok(core::ops::ControlFlow::Continue(()))
        })?;
        let len = u.arbitrary_len::<u8>()?;
        options.name = Some(u.bytes(len)?);
        Ok(options)
    }
}

impl<'k, 'n, T: sealed::FileOptionExtension> FileOptions<'k, 'n, T> {
    pub(crate) fn normalize(&mut self) {
        if !self.last_modified_time.is_valid() {
            self.last_modified_time = FileOptions::<T>::default().last_modified_time;
        }
        // The permissions set by the user
        if let Some(perms) = &mut self.permissions
            && (*perms & !0o777 == 0)
        {
            *perms |= ffi::S_IFREG;
        }
    }

    /// Indicates whether this file will be encrypted (whether with AES or `ZipCrypto`).
    pub const fn has_encryption(&self) -> bool {
        self.encrypt_with.is_some()
    }

    /// Set the compression method for the new file
    ///
    /// The default is `CompressionMethod::Deflated` if it is enabled. If not,
    /// `CompressionMethod::Bzip2` is the default if it is enabled. If neither `bzip2` nor `deflate`
    /// is enabled, [`CompressionMethod::Stored`] becomes the default and files are written uncompressed.
    #[must_use]
    pub const fn compression_method(mut self, method: CompressionMethod) -> Self {
        self.compression_method = method;
        self
    }

    /// Set the `system` field for the new file
    ///
    /// If not set, the `zip` crate will use the current system
    #[must_use]
    pub const fn system(mut self, system: System) -> Self {
        self.system = Some(system);
        self
    }

    /// Set the compression level for the new file
    ///
    /// `None` value specifies default compression level.
    ///
    /// Range of values depends on compression method:
    /// * `Deflated`: 10 - 264 for Zopfli, 0 - 9 for other encoders. Default is 24 if Zopfli is the
    ///   only encoder, or 6 otherwise.
    /// * `Bzip2`: 0 - 9. Default is 6
    /// * `Zstd`: -7 - 22, with zero being mapped to default level. Default is 3
    /// * others: only `None` is allowed
    #[must_use]
    pub const fn compression_level(mut self, level: Option<i64>) -> Self {
        self.compression_level = level;
        self
    }

    /// Set the last modified time
    ///
    /// The default is the current timestamp if the 'time' feature is enabled, and 1980-01-01
    /// otherwise
    #[must_use]
    pub const fn last_modified_time(mut self, mod_time: DateTime) -> Self {
        self.last_modified_time = mod_time;
        self
    }

    /// Set the permissions for the new file.
    ///
    /// The format is represented with unix-style permissions.
    /// The default is `0o644`, which represents `rw-r--r--` for files,
    /// and `0o755`, which represents `rwxr-xr-x` for directories.
    ///
    /// This method only preserves the file permissions bits (via a `& 0o777`) and discards
    /// higher file mode bits. So it cannot be used to denote an entry as a directory,
    /// symlink, or other special file type.
    #[must_use]
    pub const fn unix_permissions(mut self, mode: u32) -> Self {
        self.permissions = Some(mode & 0o777);
        self
    }

    /// Set the external attributes for the file.
    ///
    /// If you use both [`Self::unix_permissions`] and [`Self::external_attributes`], only external
    /// attributes are going to be used
    #[must_use]
    pub const fn external_attributes(mut self, external_perms: u32) -> Self {
        self.external_attributes = Some(external_perms);
        self
    }

    /// Set whether the new file's compressed and uncompressed size is less than 4 GiB.
    ///
    /// If set to `false` and the file exceeds the limit, an I/O error is thrown and the file is
    /// aborted. If set to `true`, readers will require ZIP64 support and if the file does not
    /// exceed the limit, 20 B are wasted. The default is `false`.
    #[must_use]
    pub const fn large_file(mut self, large: bool) -> Self {
        self.large_file = large;
        self
    }

    pub(crate) fn with_deprecated_encryption(self, password: &'k [u8]) -> FileOptions<'k, 'n, T> {
        FileOptions {
            encrypt_with: Some(EncryptWith::ZipCrypto(
                ZipCryptoKeys::derive(password),
                PhantomData,
            )),
            ..self
        }
    }

    /// Set the AES encryption parameters.
    /// The `salt` must be at least 8 bytes long for AES-128, and at least 16 bytes long for AES-256.
    /// This method is not recommended, since having a fixed salt is not secure.
    /// Consider using `with_aes_encryption` instead, which uses a random salt and is more secure.
    #[cfg(feature = "aes-crypto")]
    pub fn with_aes_encryption_and_salt(
        self,
        password: &'k [u8],
        salt: crate::aes::AesSalt,
    ) -> FileOptions<'k, 'n, T> {
        FileOptions {
            encrypt_with: Some(EncryptWith::Aes {
                mode: salt.mode(),
                password: Some(password),
                vendor_version: crate::format::aes::AesVendorVersion::Ae2,
                salt: Some(salt),
            }),
            ..self
        }
    }

    /// Set the AES encryption parameters.
    #[cfg(feature = "aes-crypto")]
    pub fn with_aes_encryption(
        self,
        mode: crate::AesMode,
        password: &'k str,
    ) -> FileOptions<'k, 'n, T> {
        self.with_aes_encryption_bytes(mode, password.as_bytes())
    }

    /// Set the AES encryption parameters.
    #[cfg(feature = "aes-crypto")]
    pub fn with_aes_encryption_bytes(
        self,
        mode: crate::AesMode,
        password: &'k [u8],
    ) -> FileOptions<'k, 'n, T> {
        FileOptions {
            encrypt_with: Some(EncryptWith::Aes {
                mode,
                password: Some(password),
                vendor_version: crate::format::aes::AesVendorVersion::Ae2,
                salt: None,
            }),
            ..self
        }
    }

    /// Sets the size of the buffer used to hold the next block that Zopfli will compress. The
    /// larger the buffer, the more effective the compression, but the more memory is required.
    /// A value of `None` indicates no buffer, which is recommended only when all non-empty writes
    /// are larger than about 32 KiB.
    #[must_use]
    #[cfg(feature = "deflate-zopfli")]
    pub const fn with_zopfli_buffer(mut self, size: Option<usize>) -> Self {
        self.zopfli_buffer_size = size;
        self
    }

    /// Returns the compression level currently set.
    pub const fn get_compression_level(&self) -> Option<i64> {
        self.compression_level
    }
    /// Sets the alignment to the given number of bytes.
    #[must_use]
    pub const fn with_alignment(mut self, alignment: u16) -> Self {
        self.alignment = alignment;
        self
    }
}
impl FileOptions<'_, '_, ExtendedFileOptions> {
    /// Check every extra field with this crate's own extra-field parser, and
    /// return an error instead of writing a field that [`ZipArchive`] would
    /// reject (for example an Info-ZIP Unicode Path field `0x7075` shorter than
    /// its 5-byte version and CRC-32 header).
    ///
    /// Opt-in and off by default, so fields are written verbatim as before:
    /// the parser rejects some fields that other software may accept. This
    /// is likely to become the default in 10.0.0.
    ///
    /// When enabled, [`Self::add_extra_field`] rejects a malformed field
    /// immediately, and starting an entry (for example with
    /// [`ZipWriter::start_file`]) rejects any malformed field added before
    /// this was enabled.
    ///
    /// [`ZipArchive`]: crate::ZipArchive
    /// [`ZipWriter::start_file`]: crate::ZipWriter::start_file
    #[must_use]
    pub fn validate_extra_fields(mut self, validate: bool) -> Self {
        self.extended_options.validate_extra_fields = validate;
        self
    }

    /// Set the file comment.
    #[must_use]
    pub fn with_file_comment<S: Into<Box<str>>>(mut self, comment: S) -> Self {
        self.extended_options.file_comment = Some(comment.into());
        self
    }

    /// Adds an extra data field.
    #[deprecated = "use add_extra_field()"]
    pub fn add_extra_data<D: AsRef<[u8]>>(
        &mut self,
        header_id: u16,
        data: D,
        central_only: bool,
    ) -> ZipResult<()> {
        self.add_extra_field(header_id, data, central_only)
    }

    /// Adds an extra field; see [`ExtendedFileOptions::add_extra_field`] for
    /// what is checked.
    pub fn add_extra_field<D: AsRef<[u8]>>(
        &mut self,
        header_id: u16,
        data: D,
        central_only: bool,
    ) -> ZipResult<()> {
        self.extended_options
            .add_extra_field(header_id, data, central_only)
    }

    /// Removes the extra fields.
    #[must_use]
    #[deprecated = "use clear_extra_fields"]
    pub fn clear_extra_data(self) -> Self {
        self.clear_extra_fields()
    }

    /// Removes the extra fields.
    #[must_use]
    pub fn clear_extra_fields(mut self) -> Self {
        if !self.extended_options.extra_fields.is_empty() {
            self.extended_options.extra_fields = Arc::new(vec![]);
        }
        self
    }
}
impl FileOptions<'static, 'static, ()> {
    /// Constructs a const `FileOptions` object.
    ///
    /// Note: This value is different than the return value of [`FileOptions::default()`]:
    ///
    /// - The `last_modified_time` is [`DateTime::DEFAULT`]. This corresponds to 1980-01-01 00:00:00
    pub const DEFAULT: Self = Self {
        compression_method: CompressionMethod::DEFAULT,
        compression_level: None,
        last_modified_time: DateTime::DEFAULT,
        large_file: false,
        permissions: None,
        encrypt_with: None,
        extended_options: (),
        alignment: 1,
        #[cfg(feature = "deflate-zopfli")]
        zopfli_buffer_size: Some(1 << 15),
        system: None,
        name: None,
        external_attributes: None,
    };
}

impl<'k, 'n> FileOptions<'k, 'n, ()> {
    /// Convert to `FullFileOptions`.
    #[must_use]
    pub fn into_full_options(self) -> FullFileOptions<'k, 'n> {
        FileOptions {
            compression_method: self.compression_method,
            compression_level: self.compression_level,
            last_modified_time: self.last_modified_time,
            permissions: self.permissions,
            large_file: self.large_file,
            encrypt_with: self.encrypt_with,
            extended_options: ExtendedFileOptions::default(),
            alignment: self.alignment,
            #[cfg(feature = "deflate-zopfli")]
            zopfli_buffer_size: self.zopfli_buffer_size,
            system: self.system,
            name: self.name,
            external_attributes: self.external_attributes,
        }
    }
}

impl<T: sealed::FileOptionExtension> Default for FileOptions<'_, '_, T> {
    /// Construct a new `FileOptions` object
    fn default() -> Self {
        Self {
            compression_method: CompressionMethod::default(),
            compression_level: None,
            last_modified_time: DateTime::default_for_write(),
            permissions: None,
            large_file: false,
            encrypt_with: None,
            extended_options: T::default(),
            alignment: 1,
            #[cfg(feature = "deflate-zopfli")]
            zopfli_buffer_size: Some(1 << 15),
            system: None,
            name: None,
            external_attributes: None,
        }
    }
}
