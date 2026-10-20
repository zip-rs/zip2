//! Like `eocd_complexity.rs`, but each EOCD points at a different entry of one central
//! directory. Work and memory must stay linear in the input length.
#![cfg(not(miri))]

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct PeakCountingAllocator;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for PeakCountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
        PEAK.fetch_max(live, Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: PeakCountingAllocator = PeakCountingAllocator;

struct ReadCountingCursor<'a> {
    inner: Cursor<&'a [u8]>,
    bytes_read: u64,
}

impl Read for ReadCountingCursor<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes_read += n as u64;
        Ok(n)
    }
}

impl Seek for ReadCountingCursor<'_> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// One junk byte, `m` headers, then `n` EOCDs, the j-th pointing at header `j % m`. With
/// `bad_last`, chains fail on a truncated custom extra field in the last header instead of at the
/// EOCDs.
fn build(m: usize, n: usize, bad_last: bool) -> Vec<u8> {
    const BAD_EXTRA_FIELD: &[u8] = b"\xff\xff\xff\xff\x00";
    let mut v = vec![0u8];
    let mut offsets = Vec::with_capacity(m);
    for i in 0..m {
        let extra: &[u8] = if bad_last && i == m - 1 {
            BAD_EXTRA_FIELD
        } else {
            b""
        };
        offsets.push((v.len() - 1) as u32);
        v.extend_from_slice(&0x0201_4b50u32.to_le_bytes()); // CDFH sig
        v.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0]); // made by, needed, flags, method
        v.extend_from_slice(&[0u8; 16]); // time/date, crc, csize, usize
        v.extend_from_slice(&[1, 0]); // name length
        v.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        v.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // comment length, disk, int attr
        v.extend_from_slice(&[0u8; 8]); // ext attr, local header offset
        v.push(b'a');
        v.extend_from_slice(extra);
    }
    let cd_size = (v.len() - 1) as u32;
    for j in 0..n {
        let first = j % m.max(1);
        let cd_offset = offsets.get(first).copied().unwrap_or(0);
        // At least 1, or `m = 0` would be a valid empty archive.
        let entries = if bad_last {
            (m - first).max(1) as u16
        } else {
            0xFFFF
        };
        v.extend_from_slice(&0x0605_4b50u32.to_le_bytes()); // EOCD sig
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(&entries.to_le_bytes());
        v.extend_from_slice(&entries.to_le_bytes());
        v.extend_from_slice(&cd_size.to_le_bytes());
        v.extend_from_slice(&cd_offset.to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes()); // comment len
    }
    v
}

#[test]
fn test_complexity_distinct_cd_offsets() {
    println!("bad_last,m,n,input_bytes,bytes_read,read_ratio,peak_heap,heap_ratio");
    for bad_last in [false, true] {
        for m in [0, 1, 1000, 4000, 16000] {
            for n in [0, 1, 500, 2000] {
                let v = build(m, n, bad_last);
                let mut reader = ReadCountingCursor {
                    inner: Cursor::new(&v[..]),
                    bytes_read: 0,
                };
                let base = LIVE.load(Relaxed);
                PEAK.store(base, Relaxed);
                let result = zip::ZipArchive::new(&mut reader);
                let peak = PEAK.load(Relaxed) - base;
                assert!(result.is_err());
                drop(result);
                println!(
                    "{bad_last},{m},{n},{},{},{:.2},{peak},{:.2}",
                    v.len(),
                    reader.bytes_read,
                    reader.bytes_read as f64 / v.len() as f64,
                    peak as f64 / v.len() as f64
                );
                assert!(reader.bytes_read <= (5 * v.len() + 1024) as u64);
                assert!(peak <= 32 * v.len() + (1 << 20));
            }
        }
    }
}
