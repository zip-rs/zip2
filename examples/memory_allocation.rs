//! Count heap allocations of a file.
//!
//! Usage:
//!   cargo run --release --example memory_allocation -- ./tests/data/files_and_dirs.zip

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{Cursor, Read, Seek, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

use zip::ZipWriter;
use zip::write::SimpleFileOptions;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static DEALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static DEALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

static TRACKING: AtomicUsize = AtomicUsize::new(0); // 0 = off, 1 = on

struct CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACKING.load(Ordering::Relaxed) == 1 {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if TRACKING.load(Ordering::Relaxed) == 1 {
            DEALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            DEALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

fn fmt_bytes(b: usize) -> String {
    if b >= 1024 * 1024 {
        format!("{:.1} MiB", b as f64 / (1024.0 * 1024.0))
    } else if b >= 1024 {
        format!("{:.1} KiB", b as f64 / 1024.0)
    } else {
        format!("{b} B")
    }
}

fn print_stats(label: &str) {
    let ac = ALLOC_COUNT.load(Ordering::Relaxed);
    let ab = ALLOC_BYTES.load(Ordering::Relaxed);
    let dc = DEALLOC_COUNT.load(Ordering::Relaxed);
    let db = DEALLOC_BYTES.load(Ordering::Relaxed);
    let net_count = ac as isize - dc as isize;
    let net_bytes = ab as isize - db as isize;
    println!("  {label}:");
    println!(
        "    allocs: {ac} ({})  deallocs: {dc} ({})",
        fmt_bytes(ab),
        fmt_bytes(db)
    );
    let sign = if net_bytes < 0 { "-" } else { "" };
    println!(
        "    net: {net_count} allocs, {}{}",
        sign,
        fmt_bytes(net_bytes.unsigned_abs())
    );
}

fn check_memory<T, R>(
    label: &str,
    value: T,
    cb: impl FnOnce(T) -> Result<R, Box<dyn std::error::Error>>,
) -> Result<R, Box<dyn std::error::Error>> {
    TRACKING.store(1, Ordering::Relaxed);
    ALLOC_COUNT.store(0, Ordering::Relaxed);
    ALLOC_BYTES.store(0, Ordering::Relaxed);
    DEALLOC_COUNT.store(0, Ordering::Relaxed);
    DEALLOC_BYTES.store(0, Ordering::Relaxed);

    let res = cb(value);
    TRACKING.store(0, Ordering::Relaxed);
    print_stats(&format!("\n{label}"));
    res
}

trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let zip_data: Box<dyn ReadSeek> = if args.len() < 2 {
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

        let zip_data = check_memory("finish()", zip_writer, |mut zip_file| {
            zip_file.write_all(b"testing")?;
            let data = zip_file.finish()?;
            Ok(data)
        })?;

        println!("\n--------------");
        Box::new(zip_data)
    } else {
        let path = &args[1];

        let file_size = std::fs::metadata(path)?.len();
        println!("File: {path}");
        println!("File size: {}", fmt_bytes(file_size as usize));

        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        Box::new(reader)
    };

    let zip_archive = check_memory("ZipArchive::new()", None, |_: Option<bool>| {
        Ok(zip::ZipArchive::new(zip_data)?)
    })?;
    let num_entries = zip_archive.len();

    let zip_archive = check_memory("by_index iteration", zip_archive, |mut archive| {
        for i in 0..num_entries {
            let _file = archive.by_index(i)?;
        }
        Ok(archive)
    })?;

    let _archive = check_memory("by_index_raw()", zip_archive, |mut archive| {
        for i in 0..num_entries {
            let _file = archive.by_index_raw(i)?;
        }
        Ok(archive.into_inner())
    })?;

    Ok(())
}
