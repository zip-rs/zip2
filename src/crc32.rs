//! Helper module to compute a CRC32 checksum

use std::io::{self, Read};

use crc32fast::Hasher;

/// Reader that validates the CRC32 when it reaches the EOF, and that fails as soon as the
/// data grows past the declared uncompressed size.
#[derive(Debug)]
pub struct Crc32Reader<R> {
    inner: R,
    hasher: Hasher,
    check: u32,
    /// Signals if `inner` stores aes encrypted data.
    /// AE-2 encrypted data doesn't use crc and sets the value to 0.
    enabled: bool,
    /// Declared uncompressed size, if known. Reading more than this is an error, so the
    /// sizes reported by the archive (e.g. `ZipArchive::decompressed_size`) bound the output.
    size_limit: Option<u64>,
    bytes_read: u64,
}

impl<R> Crc32Reader<R> {
    /// Get a new `Crc32Reader` which checks the inner reader against checksum.
    /// The check can be disabled with the arg `should_disable` (used in aes for example)
    pub(crate) fn new(inner: R, checksum: u32, should_disable: bool) -> Crc32Reader<R> {
        Crc32Reader {
            inner,
            hasher: Hasher::new(),
            check: checksum,
            enabled: !should_disable,
            size_limit: None,
            bytes_read: 0,
        }
    }

    /// Fail with an error once more than `size_limit` bytes have been read.
    pub(crate) fn with_size_limit(mut self, size_limit: Option<u64>) -> Crc32Reader<R> {
        self.size_limit = size_limit;
        self
    }

    fn count(&mut self, n: usize) -> io::Result<()> {
        self.bytes_read = self.bytes_read.saturating_add(n as u64);
        match self.size_limit {
            Some(limit) if self.bytes_read > limit => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "File is larger than its declared uncompressed size",
            )),
            _ => Ok(()),
        }
    }

    fn check_matches(&self) -> bool {
        let computed_value = self.hasher.clone().finalize();
        self.check == computed_value
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

macro_rules! invalid_checksum {
    ( $( $x:expr ),* ) => {
        io::Error::new(io::ErrorKind::InvalidData, "Invalid checksum")
    };
}

impl<R: Read> Read for Crc32Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(buf)?;
        self.count(count)?;

        if self.enabled {
            if count == 0 && !buf.is_empty() && !self.check_matches() {
                return Err(invalid_checksum!());
            }
            self.hasher.update(&buf[..count]);
        }
        Ok(count)
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        let start = buf.len();
        let n = match self.size_limit {
            // Read at most one byte past the limit, so an oversized file is detected
            // without buffering all of it.
            Some(limit) => (&mut self.inner)
                .take(limit.saturating_sub(self.bytes_read).saturating_add(1))
                .read_to_end(buf)?,
            None => self.inner.read_to_end(buf)?,
        };
        self.count(n)?;

        if self.enabled {
            self.hasher.update(&buf[start..]);
            if !self.check_matches() {
                return Err(invalid_checksum!());
            }
        }

        Ok(n)
    }

    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        let start = buf.len();
        let n = match self.size_limit {
            Some(limit) => (&mut self.inner)
                .take(limit.saturating_sub(self.bytes_read).saturating_add(1))
                .read_to_string(buf)?,
            None => self.inner.read_to_string(buf)?,
        };
        self.count(n)?;

        if self.enabled {
            self.hasher.update(&buf.as_bytes()[start..]);
            if !self.check_matches() {
                return Err(invalid_checksum!());
            }
        }

        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::Crc32Reader;

    #[test]
    fn test_empty_reader() {
        let data: &[u8] = b"";
        let mut buf = [0; 1];

        let mut reader = Crc32Reader::new(data, 0, false);
        assert_eq!(reader.read(&mut buf).unwrap(), 0);

        let mut reader = Crc32Reader::new(data, 1, false);
        assert!(
            reader
                .read(&mut buf)
                .unwrap_err()
                .to_string()
                .contains("Invalid checksum")
        );
    }

    #[test]
    fn test_byte_by_byte() {
        let data: &[u8] = b"1234";
        let mut buf = [0; 1];

        let mut reader = Crc32Reader::new(data, 0x9be_3e0a3, false);
        assert_eq!(reader.read(&mut buf).unwrap(), 1);
        assert_eq!(reader.read(&mut buf).unwrap(), 1);
        assert_eq!(reader.read(&mut buf).unwrap(), 1);
        assert_eq!(reader.read(&mut buf).unwrap(), 1);
        assert_eq!(reader.read(&mut buf).unwrap(), 0);
        // Can keep reading 0 bytes after the end
        assert_eq!(reader.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn test_zero_read() {
        let data: &[u8] = b"1234";
        let mut buf = [0; 5];

        let mut reader = Crc32Reader::new(data, 0x9be3_e0a3, false);
        assert_eq!(reader.read(&mut buf[..0]).unwrap(), 0);
        assert_eq!(reader.read(&mut buf).unwrap(), 4);
    }
}
