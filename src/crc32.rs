//! Helper module to compute a CRC32 checksum

use std::io;
use std::io::prelude::*;

use crc32fast::Hasher;

/// Reader that validates the CRC32 when it reaches the EOF.
pub struct Crc32Reader<R> {
    inner: R,
    hasher: Hasher,
    check: u32,
    /// Signals if `inner` stores aes encrypted data.
    /// AE-2 encrypted data doesn't use crc and sets the value to 0.
    enabled: bool,
}

impl<R> Crc32Reader<R> {
    /// Get a new Crc32Reader which checks the inner reader against checksum.
    /// The check is disabled if `ae2_encrypted == true`.
    pub(crate) fn new(inner: R, checksum: u32, ae2_encrypted: bool) -> Crc32Reader<R> {
        Crc32Reader {
            inner,
            hasher: Hasher::new(),
            check: checksum,
            enabled: !ae2_encrypted,
        }
    }

    fn check_matches(&self) -> bool {
        self.check == self.hasher.clone().finalize()
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

#[cold]
fn invalid_checksum() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid checksum")
}

impl<R: Read> Read for Crc32Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(buf)?;

        if self.enabled {
            if count == 0 && !buf.is_empty() && !self.check_matches() {
                return Err(invalid_checksum());
            }
            self.hasher.update(&buf[..count]);
        }
        Ok(count)
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        let start = buf.len();
        let n = self.inner.read_to_end(buf)?;

        if self.enabled {
            self.hasher.update(&buf[start..]);
            if !self.check_matches() {
                return Err(invalid_checksum());
            }
        }

        Ok(n)
    }

    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        let start = buf.len();
        let n = self.inner.read_to_string(buf)?;

        if self.enabled {
            self.hasher.update(&buf.as_bytes()[start..]);
            if !self.check_matches() {
                return Err(invalid_checksum());
            }
        }

        Ok(n)
    }
}

pub(crate) mod non_crypto {
    use std::io;
    use std::io::prelude::*;

    use crc32fast::Hasher;

    /// Reader that validates the CRC32 when it reaches the EOF.
    pub struct Crc32Reader<R> {
        inner: R,
        hasher: Hasher,
        check: u32,
    }

    impl<R> Crc32Reader<R> {
        /// Get a new Crc32Reader which checks the inner reader against checksum.
        pub(crate) fn new(inner: R, checksum: u32) -> Self {
            Crc32Reader {
                inner,
                hasher: Hasher::new(),
                check: checksum,
            }
        }

        fn check_matches(&self) -> Result<(), &'static str> {
            let res = self.hasher.clone().finalize();
            if self.check == res {
                Ok(())
            } else {
                /* TODO: make this into our own Crc32Error error type! */
                Err("Invalid checksum")
            }
        }
    }

    impl<R: Read> Read for Crc32Reader<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            /* We want to make sure we only check the hash when the input stream is exhausted. */
            if buf.is_empty() {
                /* If the input buf is empty (this shouldn't happen, but isn't guaranteed), we
                 * still want to "pull" from the source in case it surfaces an i/o error. This will
                 * always return a count of Ok(0) if successful. */
                return self.inner.read(buf);
            }

            let count = self.inner.read(buf)?;
            if count == 0 {
                return self
                    .check_matches()
                    .map(|()| 0)
                    /* TODO: use io::Error::other for MSRV >=1.74 */
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e));
            }
            self.hasher.update(&buf[..count]);
            Ok(count)
        }
    }

    #[cfg(test)]
    mod test {
        use super::*;

        #[test]
        fn test_empty_reader() {
            let data: &[u8] = b"";
            let mut buf = [0; 1];

            let mut reader = Crc32Reader::new(data, 0);
            assert_eq!(reader.read(&mut buf).unwrap(), 0);

            let mut reader = Crc32Reader::new(data, 1);
            assert!(reader
                .read(&mut buf)
                .unwrap_err()
                .to_string()
                .contains("Invalid checksum"));
        }

        #[test]
        fn test_byte_by_byte() {
            let data: &[u8] = b"1234";
            let mut buf = [0; 1];

            let mut reader = Crc32Reader::new(data, 0x9be3e0a3);
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

            let mut reader = Crc32Reader::new(data, 0x9be3e0a3);
            assert_eq!(reader.read(&mut buf[..0]).unwrap(), 0);
            assert_eq!(reader.read(&mut buf).unwrap(), 4);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_empty_reader() {
        let data: &[u8] = b"";
        let mut buf = [0; 1];

        let mut reader = Crc32Reader::new(data, 0, false);
        assert_eq!(reader.read(&mut buf).unwrap(), 0);

        let mut reader = Crc32Reader::new(data, 1, false);
        assert!(reader
            .read(&mut buf)
            .unwrap_err()
            .to_string()
            .contains("Invalid checksum"));
    }

    #[test]
    fn test_byte_by_byte() {
        let data: &[u8] = b"1234";
        let mut buf = [0; 1];

        let mut reader = Crc32Reader::new(data, 0x9be3e0a3, false);
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

        let mut reader = Crc32Reader::new(data, 0x9be3e0a3, false);
        assert_eq!(reader.read(&mut buf[..0]).unwrap(), 0);
        assert_eq!(reader.read(&mut buf).unwrap(), 4);
    }
}
