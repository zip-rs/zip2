//! Count heap allocations of a file.
//!
//! Usage:
//!   cargo run --release --example memory_allocation -- ./tests/data/files_and_dirs.zip

use core::cmp::Ordering::{Equal, Greater, Less};
use core::hint::black_box;
use core::sync::atomic::AtomicIsize;
use core::sync::atomic::Ordering::Relaxed;
use core::sync::atomic::{AtomicBool, AtomicUsize};
use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Result as IoResult;
use std::io::{BufReader, Cursor, Read, Seek, Sink, Write, copy};
use zip::read::Config;
use zip::write::SimpleFileOptions;
use zip::{ZipReadOptions, ZipWriter};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static DEALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static DEALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static REALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static REALLOC_BYTES: AtomicIsize = AtomicIsize::new(0);

static TRACKING: AtomicBool = AtomicBool::new(false);

struct PeakCountingAllocator;

unsafe impl GlobalAlloc for PeakCountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACKING.load(Relaxed) {
            let live = LIVE.fetch_add(layout.size(), Relaxed) + layout.size();
            ALLOC_COUNT.fetch_add(1, Relaxed);
            ALLOC_BYTES.fetch_add(layout.size(), Relaxed);
            PEAK.fetch_max(live, Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if TRACKING.load(Relaxed) {
            LIVE.fetch_sub(layout.size(), Relaxed);
            DEALLOC_COUNT.fetch_add(1, Relaxed);
            DEALLOC_BYTES.fetch_add(layout.size(), Relaxed);
        }
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let reallocated = unsafe { System.realloc(ptr, layout, new_size) };
        if reallocated.is_null() || !TRACKING.load(Relaxed) {
            return reallocated;
        }
        let size_change = new_size as isize - layout.size() as isize;
        match size_change.cmp(&0) {
            Less => {
                LIVE.fetch_sub(size_change.unsigned_abs(), Relaxed);
                REALLOC_COUNT.fetch_add(1, Relaxed);
                REALLOC_BYTES.fetch_add(size_change, Relaxed);
            }
            Greater => {
                let live = LIVE.fetch_add(size_change.unsigned_abs(), Relaxed)
                    + size_change.unsigned_abs();
                PEAK.fetch_max(live, Relaxed);
                REALLOC_COUNT.fetch_add(1, Relaxed);
                REALLOC_BYTES.fetch_add(size_change, Relaxed);
            }
            Equal => {}
        }
        reallocated
    }
}

#[global_allocator]
static ALLOCATOR: PeakCountingAllocator = PeakCountingAllocator;

fn fmt_bytes(b: usize) -> String {
    if b >= 1024 * 1024 {
        format!("{:.1} MiB", b as f64 / (1024.0 * 1024.0))
    } else if b >= 1024 {
        format!("{:.1} KiB", b as f64 / 1024.0)
    } else {
        format!("{b} B")
    }
}

fn check_memory<T, R>(
    label: &str,
    value: T,
    cb: impl FnOnce(T) -> Result<R, Box<dyn core::error::Error>>,
) -> Result<R, Box<dyn core::error::Error>> {
    TRACKING.store(true, Relaxed); // start tracking
    PEAK.store(0, Relaxed);
    LIVE.store(0, Relaxed);
    ALLOC_COUNT.store(0, Relaxed);
    ALLOC_BYTES.store(0, Relaxed);
    DEALLOC_COUNT.store(0, Relaxed);
    DEALLOC_BYTES.store(0, Relaxed);
    REALLOC_COUNT.store(0, Relaxed);
    REALLOC_BYTES.store(0, Relaxed);
    let base = LIVE.load(Relaxed);

    let res = cb(value);

    TRACKING.store(false, Relaxed); // stop tracking
    let ac = ALLOC_COUNT.load(Relaxed);
    let ab = ALLOC_BYTES.load(Relaxed);
    let dc = DEALLOC_COUNT.load(Relaxed);
    let db = DEALLOC_BYTES.load(Relaxed);
    let rc = REALLOC_COUNT.load(Relaxed);
    let rb = REALLOC_BYTES.load(Relaxed);
    let net_count = ac as isize - dc as isize;
    let net_bytes = ab as isize + rb - db as isize;
    println!("  {label}:");
    println!(
        "    allocs: {ac} ({})  deallocs: {dc} ({})  reallocs: {rc} ({}{})  peak usage: {}",
        fmt_bytes(ab),
        fmt_bytes(db),
        if rb < 0 { "-" } else { "" },
        fmt_bytes(rb.unsigned_abs()),
        fmt_bytes(PEAK.load(Relaxed).saturating_sub(base))
    );
    let sign = if net_bytes < 0 { "-" } else { "" };
    println!(
        "    net: {net_count} allocs, {}{}",
        sign,
        fmt_bytes(net_bytes.unsigned_abs())
    );
    res
}

trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let get_zip_data: Box<dyn Fn() -> IoResult<Box<dyn ReadSeek>>> = if args.len() < 2 {
        let zip_data = Cursor::new(Vec::with_capacity(2048));
        let zip_writer = check_memory("ZipWriter::new()", zip_data, |data| {
            let zip_writer = ZipWriter::new(data);
            Ok(zip_writer)
        })?;

        let zip_writer = check_memory("start_file()", zip_writer, |mut zip_file| {
            zip_file.start_file(
                "tests.txt",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )?;
            Ok(zip_file)
        })?;

        let zip_writer = check_memory("write()", zip_writer, |mut zip_file| {
            zip_file.write_all(b"testing")?;
            Ok(zip_file)
        })?;

        let zip_data = check_memory("finish()", zip_writer, |zip_file| {
            let data = zip_file.finish()?;
            Ok(data)
        })?;

        println!("\n--------------");
        Box::new(move || Ok(Box::new(zip_data.clone())))
    } else {
        let path = &args[1];

        let file_size = std::fs::metadata(path)?.len();
        println!("File: {path}");
        println!("File size: {}", fmt_bytes(file_size as usize));
        Box::new(move || Ok(Box::new(BufReader::new(std::fs::File::open(path)?))))
    };

    struct DecompressionSink(Sink);
    impl Write for DecompressionSink {
        #[inline(always)]
        fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
            // Enforce that the decompressed buffer contents are "used"
            let opaque_buf = black_box(buf);
            self.0.write(opaque_buf)
        }

        #[inline(always)]
        fn flush(&mut self) -> IoResult<()> {
            self.0.flush()
        }
    }
    let mut sink = DecompressionSink(std::io::sink());
    check_memory(
        "ZipIterable::new()",
        (get_zip_data()?, get_zip_data()?),
        |(header_reader, body_reader)| {
            let mut zip_iterable =
                zip::unstable::read::ZipIterable::try_new(header_reader, Config::default())?;
            let mut entries =
                zip_iterable.files_with_reader(body_reader, ZipReadOptions::default())?;
            while let Some(entry) = entries.next() {
                copy(&mut entry?, &mut sink)?;
            }
            Ok(())
        },
    )?;

    let zip_archive = check_memory("ZipArchive::new()", get_zip_data()?, |data| {
        Ok(zip::ZipArchive::new(data)?)
    })?;
    let num_entries = zip_archive.len();
    println!("Entries without exact-path duplicates: {}", num_entries);
    let zip_archive = check_memory("by_index iteration", zip_archive, |mut archive| {
        for i in 0..num_entries {
            let _file = archive.by_index(i)?;
        }
        Ok(archive)
    })?;

    let zip_archive = check_memory("by_index_raw()", zip_archive, |mut archive| {
        for i in 0..num_entries {
            let _file = archive.by_index_raw(i)?;
        }
        Ok(archive)
    })?;

    // Ensure that deallocating the ZipArchive itself is counted separately
    check_memory("into_inner()", zip_archive, |archive| {
        archive.into_inner();
        Ok(())
    })
}
