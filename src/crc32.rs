//! Helper module to compute a CRC32 checksum

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

    #[allow(dead_code)]
    pub fn into_inner(self) -> R {
        self.inner
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
