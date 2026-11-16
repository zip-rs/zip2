#[cfg(not(miri))]
struct ReadCountingCursor<'a> {
    inner: std::io::Cursor<&'a [u8]>,
    bytes_read: &'a mut u64,
}

#[cfg(not(miri))]
impl std::io::Read for ReadCountingCursor<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        *self.bytes_read += n as u64;
        Ok(n)
    }
}

#[cfg(not(miri))]
impl std::io::Seek for ReadCountingCursor<'_> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

#[cfg(not(miri))]
#[test]
fn test_complexity() {
    use std::io::Cursor;
    use std::time::Instant;

    println!("l,m,n,input_bytes,bytes_read,ratio,time_ns,ns_per_byte");
    for l in [1, 2, 4, 8] {
        for m in [0, 1, 2, 3, 4000, 8000, 16000, 32000, 64000] {
            for n in [0, 1, 2, 3, 4000, 8000, 16000, 32000, 64000] {
                if m == 0 && n == 0 && l > 1 {
                    continue;
                }
                let mut v = Vec::new();
                for _ in 0..m {
                    v.extend_from_slice(&0x0201_4b50u32.to_le_bytes()); // CDFH sig
                    v.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0]); // made by, needed, flags, method
                    v.extend_from_slice(&[0u8; 16]); // time/date, crc, csize, usize
                    v.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // name=1, extra, comment, disk, int attr
                    v.extend_from_slice(&[0u8; 8]); // ext attr, local header offset
                    v.push(b'a');
                }
                let cd_size = v.len() as u32;
                for _ in 0..n {
                    v.extend_from_slice(&0x0605_4b50u32.to_le_bytes()); // EOCD sig
                    v.extend_from_slice(&[0, 0, 0, 0]);
                    v.extend_from_slice(&((m + 1) as u16).to_le_bytes());
                    v.extend_from_slice(&((m + 1) as u16).to_le_bytes());
                    v.extend_from_slice(&cd_size.to_le_bytes());
                    v.extend_from_slice(&1u32.to_le_bytes()); // cd offset 1
                    v.extend_from_slice(&0u16.to_le_bytes()); // comment len
                }
                let v = v.repeat(l);
                let t = Instant::now();
                let mut bytes_read = 0;
                let r = zip::ZipArchive::new(ReadCountingCursor {
                    inner: Cursor::new(&v[..]),
                    bytes_read: &mut bytes_read,
                });
                let elapsed = t.elapsed().as_nanos();
                assert!(r.is_err());
                let read_ratio = bytes_read as f64 / v.len() as f64;
                let ns_per_byte = elapsed as f64 / v.len() as f64;
                println!(
                    "{l},{m},{n},{},{},{:.2},{},{:.2}",
                    v.len(),
                    bytes_read,
                    read_ratio,
                    elapsed,
                    ns_per_byte
                );
                assert!(bytes_read <= ((302 * v.len()) / 100 + 1024) as u64);
            }
        }
    }
}

/// Each EOCD candidate costs a constant number of reads (its locator, itself, and a probe of the
/// ZIP64 EOCD it points at), so the total stays a small multiple of the input length.
#[cfg(not(miri))]
#[test]
fn test_eocd64_complexity() {
    /// `records` overlapping ZIP64 EOCD signatures at a 12-byte stride, each declaring a record that
    /// runs up to the first locator, followed by `pairs` overlapping (locator, EOCD) pairs at a
    /// 24-byte stride whose locators all point the EOCD64 search at offset 0.
    fn build(records: usize, pairs: usize) -> Vec<u8> {
        let first_locator = (records * 12) as u64;
        let mut v = Vec::new();
        for i in 0..records {
            let record_size = (first_locator - (i * 12) as u64).saturating_sub(12);
            v.extend_from_slice(b"PK\x06\x06");
            v.extend_from_slice(&record_size.to_le_bytes());
        }
        v.extend_from_slice(b"PK\x06\x07");
        v.extend_from_slice(&0xffff_0000u32.to_le_bytes()); // disk with CD
        v.extend_from_slice(&0u64.to_le_bytes()); // EOCD64 offset
        v.extend_from_slice(&0u32.to_le_bytes()); // number of disks
        for _ in 0..pairs {
            v.extend_from_slice(b"PK\x05\x06");
            v.extend_from_slice(b"PK\x06\x07"); // disk numbers; also the next locator's signature
            v.extend_from_slice(&0u16.to_le_bytes());
            v.extend_from_slice(&u16::MAX.to_le_bytes()); // number of files: may be ZIP64
            v.extend_from_slice(&[0u8; 8]); // CD size and offset (the next locator's EOCD64 offset)
            v.extend_from_slice(&[0u8; 4]); // comment length, then padding
        }
        v
    }
    for (records, pairs) in [(100, 100), (1000, 500), (4000, 1000)] {
        let v = build(records, pairs);
        let mut bytes_read = 0;
        let mut reader = ReadCountingCursor {
            inner: std::io::Cursor::new(&v[..]),
            bytes_read: &mut bytes_read,
        };
        assert!(zip::ZipArchive::new(&mut reader).is_err());
        println!("{records},{pairs},{},{}", v.len(), bytes_read);
        assert!(
            bytes_read <= (8 * v.len() + 4096) as u64,
            "read {} bytes from a {}-byte input",
            bytes_read,
            v.len()
        );
    }
}
