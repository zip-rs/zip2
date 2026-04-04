// Criterion-based benchmarks (baseline storage, regression detection).
// Run: cargo bench --bench criterion_bench
// First run saves baseline; later runs compare and can fail on regression.

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use std::fs;
use std::hint::black_box;
use std::io::{self, Cursor, Read, Seek, Write};
use zip_next::{
    CompressionMethod, ZipArchive, ZipWriter, result::ZipResult, write::FileOptions,
};

// Deterministic seeded randomness helper (SplitMix64; no external dependencies, no syscalls).
// Why we use this instead of the getrandom crate:
// 1. Deterministic — always the same bytes.
// 2. No syscalls — the getrandom crate calls the OS/platform RNG.
// 3. No dependency here — getrandom is used elsewhere but remains optional and may change.
fn seeded_random_bytes(size: usize) -> Vec<u8> {
    let mut x: u64 = 0xdead_beef_cafe_babe; // seed
    let mut out = vec![0u8; size];

    for chunk in out.chunks_mut(8) {
        x = x.wrapping_add(0x9E3779B97F4A7C15);

        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;

        let bytes = z.to_le_bytes();

        for (i, b) in chunk.iter_mut().enumerate() {
            *b = bytes[i];
        }
    }

    out
}

fn generate_random_archive(size: usize) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(CompressionMethod::Stored);
    writer.start_file("random.dat", options).unwrap();

    // generate deterministic seeded random data
    let bytes = seeded_random_bytes(size);

    writer.write_all(&bytes).unwrap();
    writer.finish().unwrap().into_inner()
}

const FILE_COUNT: usize = 15_000;
const FILE_SIZE_META: usize = 1024;

fn generate_random_archive_meta(count_files: usize, file_size: usize) -> ZipResult<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(CompressionMethod::Stored);

    // seeded random payload reused across entries
    let bytes = seeded_random_bytes(file_size);

    for i in 0..count_files {
        let name = format!("file_deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef_{i}.dat");
        writer.start_file(name, options.clone())?;
        writer.write_all(&bytes)?;
    }

    Ok(writer.finish()?.into_inner())
}

fn generate_random_archive_merge(
    num_entries: usize,
    entry_size: usize,
    options: FileOptions,
) -> ZipResult<(usize, Vec<u8>)> {
    let buf = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);

    // seeded random payload reused across entries
    let bytes = seeded_random_bytes(entry_size);

    for i in 0..num_entries {
        let name = format!("random{i}.dat");
        zip.start_file(name, options.clone())?;
        zip.write_all(&bytes)?;
    }

    let buf = zip.finish()?.into_inner();
    let len = buf.len();

    Ok((len, buf))
}

fn perform_raw_copy_file<R: Read + Seek, W: Write + Seek>(
    mut src: ZipArchive<R>,
    mut target: ZipWriter<W>,
) -> ZipResult<ZipWriter<W>> {
    for i in 0..src.len() {
        let entry = src.by_index(i)?;
        target.raw_copy_file(entry)?;
    }
    Ok(target)
}

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

fn generate_archive_with_comment(comment_len: usize) -> ZipResult<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = FileOptions::default().compression_method(CompressionMethod::Stored);
    let comment = seeded_random_bytes(comment_len);
    writer.set_raw_comment(comment);
    writer.start_file("data.txt", options)?;
    writer.write_all(b"x")?;
    Ok(writer.finish()?.into_inner())
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
    fs::write(
        &path_read_entry,
        generate_random_archive(1024 * 1024),
    )
        .unwrap();

    c.bench_function("read_entry", |b| {
        b.iter(|| {
            let mut archive = ZipArchive::new(fs::File::open(&path_read_entry).unwrap()).unwrap();
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
    // merge_archive_raw_copy_file_stored
    // Second merge source (independent); raw_copy_file path.
    // ============================================================================
    let merge_options =
        FileOptions::default().compression_method(CompressionMethod::Stored);
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
                let mut zip = perform_raw_copy_file(src, zip).unwrap();
                let out = zip.finish().unwrap();
                assert_eq!(out.metadata().unwrap().len() as usize, len2);
                black_box(out);
            },
            BatchSize::SmallInput,
        );
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
            while zip_next::read::read_zipfile_from_stream(&mut f)
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

    c.bench_function("write_many_small_files", |b| {
        b.iter(|| {
            let mut writer = ZipWriter::new(fs::File::create(&path_write_many).unwrap());
            let options =
                FileOptions::default().compression_method(CompressionMethod::Stored);
            for i in 0..WRITE_MANY_FILES {
                let name = format!("file_{i}.dat");
                writer.start_file(name, options.clone()).unwrap();
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
                FileOptions::default().compression_method(CompressionMethod::Stored);
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
                FileOptions::default().compression_method(CompressionMethod::Stored);
            for i in 0..ROUNDTRIP_ENTRIES {
                writer
                    .start_file(format!("entry_{i}.dat"), options.clone())
                    .unwrap();
                writer.write_all(&roundtrip_payload).unwrap();
            }
            drop(writer.finish().unwrap());
            let archive = ZipArchive::new(fs::File::open(&path_roundtrip).unwrap()).unwrap();
            black_box(archive.len());
        });
    });

    #[cfg(feature = "deflate")]
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
                FileOptions::default().compression_method(CompressionMethod::Deflated),
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

                let options =
                    FileOptions::default().compression_method(CompressionMethod::Deflated);

                for i in 0..20 {
                    writer
                        .start_file(format!("deflated_{i}.dat"), options.clone())
                        .unwrap();
                    writer.write_all(&deflate_payload).unwrap();
                }

                black_box(writer.finish().unwrap());
            });
        });
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
