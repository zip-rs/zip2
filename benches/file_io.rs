//! Criterion-based benchmarks.
//!
//! Usage:
//!
//! ```sh
//! cargo bench --bench file_io
//!
//! # optional baselines
//!
//! cargo bench --bench file_io -- --save-baseline my_version
//! cargo bench --bench file_io -- --baseline my_version
//! ```
//!
//! Merge / raw_copy benches: longer measurement (10s) so samples complete on slow hosts (e.g. SBCs).

#[cfg(not(any(miri, target_arch = "wasm32")))]
mod common;

#[cfg(not(any(miri, target_arch = "wasm32")))]
mod benches {
    use super::common::{
        generate_archive_with_comment, generate_random_archive, generate_random_archive_merge,
        generate_random_archive_meta, perform_raw_copy_file_by_index,
        seeded_random_bytes,
    };
    use criterion::{BatchSize, Criterion, criterion_group};
    use std::fs;
    use std::hint::black_box;
    use std::io::{self, Read, Write};
    use std::time::Duration;
    use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

    const FILE_COUNT: usize = 15_000;
    const FILE_SIZE_META: usize = 1024;

    const NUM_ENTRIES: usize = 100;
    const ENTRY_SIZE: usize = 1024;

    const COMMENT_BENCH_LEN: usize = 50_000;
    const READ_ALL_ENTRIES_FILES: usize = 500;
    const BY_NAME_LOOKUP_COUNT: usize = 50;
    const LARGE_NON_ZIP_BYTES: usize = 17_000_000;
    const WRITE_MANY_FILES: usize = 1_000;

    const STREAM_ENTRIES: usize = 20;
    const STREAM_ENTRY_SIZE: usize = 256;
    const WRITE_LARGE_SIZE: usize = 1024 * 1024;
    const ROUNDTRIP_ENTRIES: usize = 100;

    /// Uses a longer Criterion window than the default for stable timing on slow hosts.
    fn merge_archive_benchmarks(c: &mut Criterion) {
        let bench_dir = tempfile::TempDir::with_prefix("criterion_zip_merge").unwrap();
        let p = |name: &str| bench_dir.path().join(name);

        let merge_options =
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let path_merge_src = p("merge_src.zip");
        let path_merge_out = p("merge_out.zip");
        let (len, src_bytes) =
            generate_random_archive_merge(NUM_ENTRIES, ENTRY_SIZE, merge_options).unwrap();
        fs::write(&path_merge_src, &src_bytes).unwrap();

        c.bench_function("merge_archive_stored", |b| {
            b.iter_batched(
                || {
                    let src = ZipArchive::new(fs::File::open(&path_merge_src).unwrap()).unwrap();
                    let out = fs::File::create(&path_merge_out).unwrap();
                    (src, out)
                },
                |(src, out)| {
                    let mut zip = ZipWriter::new(out);
                    zip.merge_archive(src).unwrap();
                    let out = zip.finish().unwrap();
                    assert_eq!(out.metadata().unwrap().len() as usize, len);
                    black_box(out);
                },
                BatchSize::PerIteration,
            );
        });

        let path_merge_src2 = p("merge_src2.zip");
        let path_merge_out2 = p("merge_out2.zip");
        let (len2, src_bytes2) =
            generate_random_archive_merge(NUM_ENTRIES, ENTRY_SIZE, merge_options).unwrap();
        fs::write(&path_merge_src2, &src_bytes2).unwrap();

        c.bench_function("merge_archive_raw_copy_file_stored", |b| {
            b.iter_batched(
                || {
                    let src = ZipArchive::new(fs::File::open(&path_merge_src2).unwrap()).unwrap();
                    let out = fs::File::create(&path_merge_out2).unwrap();
                    (src, out)
                },
                |(src, out)| {
                    let zip = ZipWriter::new(out);
                    let zip = perform_raw_copy_file_by_index(src, zip).unwrap();
                    let out = zip.finish().unwrap();
                    assert_eq!(out.metadata().unwrap().len() as usize, len2);
                    black_box(out);
                },
                BatchSize::PerIteration,
            );
        });
    }

    fn criterion_benchmark(c: &mut Criterion) {
        // Shared directory: all fixtures use real files under here (see each section below).
        let bench_dir = tempfile::TempDir::with_prefix("criterion_zip").unwrap();
        let p = |name: &str| bench_dir.path().join(name);

        // ============================================================================
        // read_entry
        // Single stored entry (~1 MiB payload); read full entry in a loop from disk.
        // ============================================================================
        let path_read_entry = p("read_entry.zip");
        fs::write(&path_read_entry, generate_random_archive(1024 * 1024)).unwrap();

        c.bench_function("read_entry", |b| {
            b.iter(|| {
                let mut archive =
                    ZipArchive::new(fs::File::open(&path_read_entry).unwrap()).unwrap();
                let mut file = archive.by_name("random.dat").unwrap();
                let mut buf = [0u8; 1024];

                loop {
                    let n = file.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                }

                black_box(buf);
            });
        });

        // ============================================================================
        // read_metadata / by_name_lookup_many
        // One large archive on disk (FILE_COUNT × FILE_SIZE_META); shared by both benches.
        // ============================================================================
        let path_meta = p("meta.zip");
        fs::write(
            &path_meta,
            generate_random_archive_meta(FILE_COUNT, FILE_SIZE_META).unwrap(),
        )
        .unwrap();

        c.bench_function("read_metadata", |b| {
            b.iter(|| {
                black_box(
                    ZipArchive::new(fs::File::open(&path_meta).unwrap())
                        .unwrap()
                        .len(),
                )
            });
        });

        // ============================================================================
        // read_all_entries
        // Many small entries; read each by index to sink (from disk).
        // ============================================================================
        let path_all_entries = p("all_entries.zip");
        fs::write(
            &path_all_entries,
            generate_random_archive_meta(READ_ALL_ENTRIES_FILES, 512).unwrap(),
        )
        .unwrap();

        c.bench_function("read_all_entries", |b| {
            b.iter(|| {
                let mut archive =
                    ZipArchive::new(fs::File::open(&path_all_entries).unwrap()).unwrap();
                for i in 0..archive.len() {
                    let mut entry = archive.by_index(i).unwrap();
                    let _ = io::copy(&mut entry, &mut io::sink()).unwrap();
                }
            });
        });

        // ============================================================================
        // by_name_lookup_many
        // Uses meta.zip above; repeated by_name lookups (names prebuilt once).
        // ============================================================================
        let lookup_names: Vec<String> = (0..BY_NAME_LOOKUP_COUNT)
            .map(|i| format!("file_deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef_{i}.dat"))
            .collect();
        c.bench_function("by_name_lookup_many", |b| {
            b.iter(|| {
                let mut archive = ZipArchive::new(fs::File::open(&path_meta).unwrap()).unwrap();
                for name in &lookup_names {
                    let _ = archive.by_name(name).unwrap();
                }
            });
        });

        // ============================================================================
        // parse_archive_with_comment
        // Zip with large comment field; parse and read comment length from disk.
        // ============================================================================
        let path_comment = p("comment.zip");
        fs::write(
            &path_comment,
            generate_archive_with_comment(COMMENT_BENCH_LEN).unwrap(),
        )
        .unwrap();

        c.bench_function("parse_archive_with_comment", |b| {
            b.iter(|| {
                let archive = ZipArchive::new(fs::File::open(&path_comment).unwrap()).unwrap();
                black_box(archive.comment().len());
            });
        });

        // ============================================================================
        // read_stream_entries
        // Streaming API: read_zipfile_from_stream until None (file on disk).
        // ============================================================================
        let path_stream = p("stream.zip");
        fs::write(
            &path_stream,
            generate_random_archive_meta(STREAM_ENTRIES, STREAM_ENTRY_SIZE).unwrap(),
        )
        .unwrap();

        c.bench_function("read_stream_entries", |b| {
            b.iter(|| {
                let mut f = fs::File::open(&path_stream).unwrap();
                while zip::read::read_zipfile_from_stream(&mut f)
                    .unwrap()
                    .is_some()
                {}
            });
        });

        let path_small_stream = p("small_stream.zip");
        fs::write(
            &path_small_stream,
            generate_random_archive_meta(5, 5).unwrap(),
        )
        .unwrap();
        c.bench_function("parse_stream_archive_small", |b| {
            b.iter(|| {
                let mut f = fs::File::open(&path_small_stream).unwrap();
                while zip::read::read_zipfile_from_stream(&mut f)
                    .unwrap()
                    .is_some()
                {}
            });
        });

        // ============================================================================
        // parse_large_non_zip_reject
        // Large non-zip blob on disk; ZipArchive::new must fail (reject path).
        // ============================================================================
        let path_reject = p("zeros.bin");
        fs::write(&path_reject, vec![0u8; LARGE_NON_ZIP_BYTES]).unwrap();

        c.bench_function("parse_large_non_zip_reject", |b| {
            b.iter(|| {
                let r = ZipArchive::new(fs::File::open(&path_reject).unwrap());
                assert!(r.is_err());
            });
        });

        // ============================================================================
        // write_many_small_files
        // Payload in memory; write many stored entries to a file (truncated each iter).
        // ============================================================================
        let path_write_many = p("write_many.zip");
        let payload_small = seeded_random_bytes(128);
        let file_names: Vec<_> = (0..WRITE_MANY_FILES)
            .map(|i| format!("file_{i}.dat"))
            .collect();

        c.bench_function("write_many_small_files", |b| {
            b.iter(|| {
                let mut writer = ZipWriter::new(fs::File::create(&path_write_many).unwrap());
                let options =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
                for name in file_names.iter() {
                    writer.start_file(name, options).unwrap();
                    writer.write_all(&payload_small).unwrap();
                }
                black_box(writer.finish().unwrap());
            });
        });

        // ============================================================================
        // write_one_large_file
        // ~1 MiB stored payload; single entry written to disk each iter.
        // ============================================================================
        let path_write_large = p("write_large.zip");
        let payload_large = seeded_random_bytes(WRITE_LARGE_SIZE);

        c.bench_function("write_one_large_file", |b| {
            b.iter(|| {
                let mut writer = ZipWriter::new(fs::File::create(&path_write_large).unwrap());
                let options =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
                writer.start_file("large.dat", options).unwrap();
                writer.write_all(&payload_large).unwrap();
                black_box(writer.finish().unwrap());
            });
        });

        // ============================================================================
        // write_then_read_roundtrip
        // Write several entries to disk, reopen, read archive length.
        // ============================================================================
        let path_roundtrip = p("roundtrip.zip");
        let roundtrip_payload = seeded_random_bytes(256);

        c.bench_function("write_then_read_roundtrip", |b| {
            b.iter(|| {
                let mut writer = ZipWriter::new(fs::File::create(&path_roundtrip).unwrap());
                let options =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
                for i in 0..ROUNDTRIP_ENTRIES {
                    writer
                        .start_file(format!("entry_{i}.dat"), options)
                        .unwrap();
                    writer.write_all(&roundtrip_payload).unwrap();
                }
                drop(writer.finish().unwrap());
                let archive = ZipArchive::new(fs::File::open(&path_roundtrip).unwrap()).unwrap();
                black_box(archive.len());
            });
        });

        #[cfg(feature = "deflate-flate2")]
        {
            // ============================================================================
            // read_deflated_entry
            // Deflated entries on disk; read all to sink.
            // ============================================================================
            let path_deflate_read = p("deflate_read.zip");
            fs::write(
                &path_deflate_read,
                generate_random_archive_merge(
                    5,
                    4096,
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
                )
                .unwrap()
                .1,
            )
            .unwrap();

            c.bench_function("read_deflated_entry", |b| {
                b.iter(|| {
                    let mut archive =
                        ZipArchive::new(fs::File::open(&path_deflate_read).unwrap()).unwrap();
                    for i in 0..archive.len() {
                        let mut entry = archive.by_index(i).unwrap();
                        let _ = io::copy(&mut entry, &mut io::sink()).unwrap();
                    }
                });
            });

            // ============================================================================
            // write_deflated_entries
            // Deflated writes to disk each iter.
            // ============================================================================
            let path_deflate_write = p("deflate_write.zip");
            let deflate_payload = seeded_random_bytes(2048);

            c.bench_function("write_deflated_entries", |b| {
                b.iter(|| {
                    let mut writer = ZipWriter::new(fs::File::create(&path_deflate_write).unwrap());

                    let options = SimpleFileOptions::default()
                        .compression_method(CompressionMethod::Deflated);

                    for i in 0..20 {
                        writer
                            .start_file(format!("deflated_{i}.dat"), options)
                            .unwrap();
                        writer.write_all(&deflate_payload).unwrap();
                    }

                    black_box(writer.finish().unwrap());
                });
            });
        }
    }

    criterion_group! {
        name = merge_archive_benches;
        config = Criterion::default()
            .measurement_time(Duration::from_secs(10))
            .warm_up_time(Duration::from_secs(4));
        targets = merge_archive_benchmarks
    }
    criterion_group!(benches, criterion_benchmark);
}
#[cfg(not(any(miri, target_arch = "wasm32")))]
criterion::criterion_main!(benches::merge_archive_benches, benches::benches);
#[cfg(any(miri, target_arch = "wasm32"))]
fn main() {
    eprintln!("file_io does not compile on miri or wasm32 targets; skipping it");
}
