#![doc = "Unstable APIs\n\
\
All APIs accessible by importing this module are unstable; They may be changed in patch \
releases. You MUST use an exact version specifier in `Cargo.toml`, to indicate the version of this \
API you're using:\n\
\
```toml\n
[dependencies]\n
zip = \"="]
#![doc=env!("CARGO_PKG_VERSION")]
#![doc = "\"\n\
```"]

use std::borrow::Cow;
use std::io;
use std::io::{Read, Write};
use std::path::{Component, MAIN_SEPARATOR, Path};

/// Provides high level API for reading from a stream.
pub mod stream {
    pub use crate::read::stream::{ZipStreamFileMetadata, ZipStreamReader, ZipStreamVisitor};
}
/// Types for creating ZIP archives.
pub mod write {
    use crate::result::{ZipError, ZipResult};
    use crate::write::{FileOptionExtension, FileOptions};
    /// Unstable methods for [`FileOptions`].
    pub trait FileOptionsExt<'k> {
        /// Write the file with the given password using the deprecated `ZipCrypto` algorithm.
        ///
        /// <div class="warning">This is not recommended for new archives, as `ZipCrypto` is not
        /// secure (it can be cracked given 12 bytes of known plaintext after compression). It is
        /// provided only for backward compatibility with older software that doesn't support
        /// AES-encrypted archives.</div>
        fn with_deprecated_encryption(self, password: &'k [u8]) -> ZipResult<Self>
        where
            Self: Sized;
    }
    impl<'k, 'n, T: FileOptionExtension> FileOptionsExt<'k> for FileOptions<'k, 'n, T> {
        fn with_deprecated_encryption(
            self,
            password: &'k [u8],
        ) -> ZipResult<FileOptions<'k, 'n, T>> {
            if password.is_empty() {
                // Forbid empty passwords to avoid deriving a predictable key from a
                // fixed, public value. Callers must ensure passwords are non-empty.
                // This check isn't in with_deprecated_encryption itself because that would also
                // affect reading.
                Err(ZipError::InvalidPassword)
            } else {
                Ok(self.with_deprecated_encryption(password))
            }
        }
    }
}

/// Helper methods for writing unsigned integers in little-endian form.
pub trait LittleEndianWriteExt: Write {
    /// Write a u16 as little endian
    fn write_u16_le(&mut self, input: u16) -> io::Result<()> {
        self.write_all(&input.to_le_bytes())
    }
    /// Write a u32 as little endian
    fn write_u32_le(&mut self, input: u32) -> io::Result<()> {
        self.write_all(&input.to_le_bytes())
    }

    /// Write a u64 as little endian
    fn write_u64_le(&mut self, input: u64) -> io::Result<()> {
        self.write_all(&input.to_le_bytes())
    }

    /// Write a u128 as little endian
    fn write_u128_le(&mut self, input: u128) -> io::Result<()> {
        self.write_all(&input.to_le_bytes())
    }
}

impl<W: Write + ?Sized> LittleEndianWriteExt for W {}

/// Helper methods for reading unsigned integers in little-endian form.
pub trait LittleEndianReadExt: Read {
    /// Read a u16 as little endian
    fn read_u16_le(&mut self) -> io::Result<u16> {
        let mut out = [0u8; 2];
        self.read_exact(&mut out)?;
        Ok(u16::from_le_bytes(out))
    }

    /// Read a u32 as little endian
    fn read_u32_le(&mut self) -> io::Result<u32> {
        let mut out = [0u8; 4];
        self.read_exact(&mut out)?;
        Ok(u32::from_le_bytes(out))
    }

    /// Read a u64 as little endian
    fn read_u64_le(&mut self) -> io::Result<u64> {
        let mut out = [0u8; 8];
        self.read_exact(&mut out)?;
        Ok(u64::from_le_bytes(out))
    }
}

impl<R: Read> LittleEndianReadExt for R {}

/// Converts a path to the ZIP format (forward-slash-delimited and normalized).
pub fn path_to_string<T: AsRef<Path>>(path: T) -> Result<Box<str>, std::io::Error> {
    let mut maybe_original = None;
    if let Some(original) = path.as_ref().to_str() {
        if original.is_empty() || original == "." || original == ".." {
            return Ok(String::new().into_boxed_str());
        }
        if original.starts_with(MAIN_SEPARATOR) {
            if original.len() == 1 {
                return Ok(MAIN_SEPARATOR.to_string().into_boxed_str());
            } else if (MAIN_SEPARATOR == '/' || !original[1..].contains(MAIN_SEPARATOR))
                && !original.ends_with('.')
                && !original.contains([MAIN_SEPARATOR, MAIN_SEPARATOR])
                && !original.contains([MAIN_SEPARATOR, '.', MAIN_SEPARATOR])
                && !original.contains([MAIN_SEPARATOR, '.', '.', MAIN_SEPARATOR])
            {
                maybe_original = Some(&original[1..]);
            }
        } else if !original.contains(MAIN_SEPARATOR) {
            return Ok(original.into());
        }
    }
    let mut recreate = maybe_original.is_none();
    let mut normalized_components = Vec::new();

    for component in path.as_ref().components() {
        match component {
            Component::Normal(os_str) => {
                if let Some(valid_str) = os_str.to_str() {
                    normalized_components.push(Cow::Borrowed(valid_str));
                } else {
                    recreate = true;
                    normalized_components.push(os_str.to_string_lossy());
                }
            }
            Component::ParentDir => {
                recreate = true;
                normalized_components.pop();
            }
            _ => {
                recreate = true;
            }
        }
    }
    if recreate {
        Ok(normalized_components.join("/").into())
    } else {
        Ok(maybe_original
            .ok_or_else(|| std::io::Error::other("Original path is empty"))?
            .into())
    }
}
