//! Regression tests for <https://github.com/zip-rs/zip2/issues/633>: `ZipWriter` must reuse its
//! DEFLATE compressor across entries instead of allocating a new one (~400 KiB with zlib-rs) for
//! each, and doing so must not change the compressed bytes.
#![cfg(feature = "deflate-flate2")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::{Cursor, Read, Write};
use zip::unstable::write::FileOptionsExt;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// Counts bytes allocated by the current thread, so that tests running in parallel don't
/// disturb each other's measurements.
struct CountingAlloc;

thread_local! {
    static ALLOCATED: Cell<u64> = const { Cell::new(0) };
}

fn count(size: usize) {
    // `try_with` because the TLS slot may already be gone during thread teardown.
    let _ = ALLOCATED.try_with(|a| a.set(a.get() + size as u64));
}

#[cfg(not(miri))]
const ENTRIES: usize = 100;

#[cfg(miri)]
const ENTRIES: usize = 10;

#[cfg(not(miri))]
const RUN_LENGTH: usize = 16384;

#[cfg(miri)]
const RUN_LENGTH: usize = 128;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn allocated() -> u64 {
    ALLOCATED.with(Cell::get)
}

fn entry_data(i: usize, len: usize) -> Vec<u8> {
    (0..len)
        .map(|j| b"zip2 deflate reuse "[(i + j) % 19] ^ ((j / 97) as u8))
        .collect()
}

fn write_archive(entries: usize, options: SimpleFileOptions) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::with_capacity(1 << 20)));
    for i in 0..entries {
        zip.start_file(format!("{i}.txt"), options).unwrap();
        zip.write_all(&entry_data(i, RUN_LENGTH)).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn deflate_compressor_is_reused_across_entries() {
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6));
    // Warm up anything lazily initialised, then compare the marginal cost of 100 extra entries.
    let _ = write_archive(2, options);
    let before = allocated();
    let _ = write_archive(2, options);
    let small = allocated() - before;
    let before = allocated();
    let _ = write_archive(ENTRIES, options);
    let large = allocated() - before;
    let per_entry = (large - small) / (ENTRIES as u64 - 2);
    // A fresh zlib-rs/miniz_oxide compressor costs well over 200 KiB; per-entry bookkeeping
    // (names, metadata, central directory growth) is a few KiB.
    assert!(
        per_entry < 64 * 1024,
        "each DEFLATE entry allocated {per_entry} bytes; is the compressor being rebuilt?"
    );
}

/// Each entry must hold exactly the raw DEFLATE stream a fresh `flate2` encoder would produce,
/// whatever the previous entries' levels and methods were.
#[test]
fn reused_compressor_output_matches_fresh_encoder() {
    let plan: &[(CompressionMethod, Option<i64>)] = &[
        (CompressionMethod::Deflated, Some(9)),
        (CompressionMethod::Deflated, Some(9)),
        (CompressionMethod::Deflated, Some(1)),
        (CompressionMethod::Stored, None),
        (CompressionMethod::Deflated, None),
        (CompressionMethod::Deflated, Some(1)),
        (CompressionMethod::Deflated, Some(6)),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let mut expected = Vec::new();
    for (i, &(method, level)) in plan.iter().enumerate() {
        let data = entry_data(i, 50_000 + 1_000 * i);
        let options = SimpleFileOptions::default()
            .compression_method(method)
            .compression_level(level);
        zip.start_file(format!("{i}"), options).unwrap();
        zip.write_all(&data).unwrap();
        if i == 2 {
            // An aborted entry leaves the compressor mid-stream; the next one must start clean.
            zip.write_all(&entry_data(99, 10_000)).unwrap();
            zip.abort_file().unwrap();
            zip.start_file(format!("{i}"), options).unwrap();
            zip.write_all(&data).unwrap();
        }
        let raw = if method == CompressionMethod::Deflated {
            let level = flate2::Compression::new(level.unwrap_or(6) as u32);
            let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), level);
            enc.write_all(&data).unwrap();
            enc.finish().unwrap()
        } else {
            data.clone()
        };
        expected.push((data, raw));
    }
    let bytes = zip.finish().unwrap().into_inner();

    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    assert_eq!(archive.len(), plan.len());
    for (i, (data, raw)) in expected.iter().enumerate() {
        let mut actual_raw = Vec::new();
        archive
            .by_index_raw(i)
            .unwrap()
            .read_to_end(&mut actual_raw)
            .unwrap();
        assert_eq!(&actual_raw, raw, "raw bytes of entry {i} differ");
        let mut actual = Vec::new();
        archive
            .by_index(i)
            .unwrap()
            .read_to_end(&mut actual)
            .unwrap();
        assert_eq!(&actual, data, "decompressed entry {i} differs");
    }
}

/// Writes `plan` (options, is-encrypted) as Deflated entries through one `ZipWriter`, so that the
/// reused encoder's inner writer is swapped between plain and encrypting writers, then checks
/// every entry round-trips.
fn round_trip_mixed_encryption(plan: &[(SimpleFileOptions, bool)]) {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (i, (options, _)) in plan.iter().enumerate() {
        zip.start_file(format!("{i}"), *options).unwrap();
        zip.write_all(&entry_data(i, RUN_LENGTH)).unwrap();
    }
    let bytes = zip.finish().unwrap().into_inner();

    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    for (i, (_, encrypted)) in plan.iter().enumerate() {
        let mut out = Vec::new();
        if *encrypted {
            archive
                .by_index_decrypt(i, b"hunter2")
                .unwrap()
                .read_to_end(&mut out)
                .unwrap();
        } else {
            archive.by_index(i).unwrap().read_to_end(&mut out).unwrap();
        }
        assert_eq!(out, entry_data(i, RUN_LENGTH), "entry {i}");
    }
}

fn deflated() -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6))
}

#[test]
fn reused_compressor_with_zipcrypto_entries_round_trips() {
    let plain = deflated();
    let zipcrypto = plain.with_deprecated_encryption(b"hunter2").unwrap();
    round_trip_mixed_encryption(&[
        (plain, false),
        (zipcrypto, true),
        (zipcrypto, true),
        (plain, false),
    ]);
}

#[cfg(feature = "aes-crypto")]
#[test]
fn reused_compressor_with_aes_entries_round_trips() {
    let plain = deflated();
    let zipcrypto = plain.with_deprecated_encryption(b"hunter2").unwrap();
    let aes = plain.with_aes_encryption(zip::AesMode::Aes256, "hunter2");
    round_trip_mixed_encryption(&[
        (plain, false),
        (aes, true),
        (zipcrypto, true),
        (aes, true),
        (plain, false),
        (aes, true),
    ]);
}
