// Cargo.toml: zip = "=8.6.0"
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::time::Instant;

struct ReadCountingCursor<'a> {
    inner: Cursor<&'a [u8]>,
    bytes_read: &'a mut u64,
}

impl Read for ReadCountingCursor<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        *self.bytes_read += n as u64;
        Ok(n)
    }
}

impl Seek for ReadCountingCursor<'_> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

#[cfg(not(miri))]
#[test]
fn test_complexity() {
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
