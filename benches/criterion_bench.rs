// Criterion-based benchmarks (baseline storage, regression detection).
// Run: cargo bench --bench criterion_bench
// First run saves baseline; later runs compare and can fail on regression.

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use std::fs;
use std::hint::black_box;
use std::io::{self, Cursor, Read, Seek, Write};
use zip::{result::ZipResult, write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

// deterministic seeded randomness helper (SplitMix64, no external dependencies)
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
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
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
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    // seeded random payload reused across entries
    let bytes = seeded_random_bytes(file_size);

    for i in 0..count_files {
        let name = format!("file_deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef_{i}.dat");
        writer.start_file(name, options)?;
        writer.write_all(&bytes)?;
    }

    Ok(writer.finish()?.into_inner())
}

fn generate_random_archive_merge(
    num_entries: usize,
    entry_size: usize,
    options: SimpleFileOptions,
) -> ZipResult<(usize, Vec<u8>)> {
    let buf = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buf);

    // seeded random payload reused across entries
    let bytes = seeded_random_bytes(entry_size);

    for i in 0..num_entries {
        let name = format!("random{i}.dat");
        zip.start_file(name, options)?;
        zip.write_all(&bytes)?;
    }

    let buf = zip.finish()?.into_inner();
    let len = buf.len();

    Ok((len, buf))
}

fn perform_merge<R: Read + Seek, W: Write + Seek>(
    src: ZipArchive<R>,
    mut target: ZipWriter<W>,
) -> ZipResult<ZipWriter<W>> {
    target.merge_archive(src)?;
    Ok(target)
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

// Default sizes (desktop). When BENCH_PI=1 or BENCH_LOW_MEMORY=1, use smaller sizes for Pi 3B (~1 GB RAM).
fn is_low_memory() -> bool {
    std::env::var("BENCH_PI").as_deref() == Ok("1")
        || std::env::var("BENCH_LOW_MEMORY").as_deref() == Ok("1")
}

fn file_count_meta() -> usize {
    if is_low_memory() {
        2_000 // ~2 MB archive instead of ~15 MB
    } else {
        FILE_COUNT
    }
}

fn comment_size() -> usize {
    if is_low_memory() {
        10_000
    } else {
        50_000
    }
}

fn read_all_entries_count() -> usize {
    if is_low_memory() {
        200
    } else {
        500
    }
}

fn by_name_lookup_count() -> usize {
    if is_low_memory() {
        20
    } else {
        50
    }
}

fn large_non_zip_size() -> usize {
    if is_low_memory() {
        5_000_000 // 5 MB instead of 17 MB
    } else {
        17_000_000
    }
}

fn write_many_count() -> usize {
    if is_low_memory() {
        300
    } else {
        1_000
    }
}

const STREAM_ENTRIES: usize = 20;
const STREAM_ENTRY_SIZE: usize = 256;
const WRITE_LARGE_SIZE: usize = 1024 * 1024;
const ROUNDTRIP_ENTRIES: usize = 100;

fn generate_archive_with_comment(comment_len: usize) -> ZipResult<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let comment = seeded_random_bytes(comment_len);
    writer.set_raw_comment(comment.into_boxed_slice());
    writer.start_file("data.txt", options)?;
    writer.write_all(b"x")?;
    Ok(writer.finish()?.into_inner())
}

fn criterion_benchmark(c: &mut Criterion) {
    let size = 1024 * 1024;
    let bytes = generate_random_archive(size);

    c.bench_function("read_entry", |b| {
        b.iter(|| {
            let mut archive = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
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

    let bytes_meta = generate_random_archive_meta(file_count_meta(), FILE_SIZE_META).unwrap();

    c.bench_function("read_metadata", |b| {
        b.iter(|| {
            black_box(
                ZipArchive::new(Cursor::new(bytes_meta.as_slice()))
                    .unwrap()
                    .len(),
            )
        });
    });

    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    let (len, src_bytes) = generate_random_archive_merge(NUM_ENTRIES, ENTRY_SIZE, options).unwrap();

    c.bench_function("merge_archive_stored", |b| {
        b.iter_batched(
            || {
                let src = ZipArchive::new(Cursor::new(src_bytes.clone())).unwrap();
                let buf = Cursor::new(Vec::with_capacity(len));
                (src, buf)
            },
            |(src, buf)| {
                let zip = ZipWriter::new(buf);
                let zip = perform_merge(src, zip).unwrap();
                let out = zip.finish().unwrap().into_inner();

                assert_eq!(out.len(), len);

                black_box(out)
            },
            BatchSize::SmallInput,
        );
    });

    let (len2, src_bytes2) =
        generate_random_archive_merge(NUM_ENTRIES, ENTRY_SIZE, options).unwrap();

    c.bench_function("merge_archive_raw_copy_file_stored", |b| {
        b.iter_batched(
            || {
                let src = ZipArchive::new(Cursor::new(src_bytes2.clone())).unwrap();
                let buf = Cursor::new(Vec::with_capacity(len2));
                (src, buf)
            },
            |(src, buf)| {
                let zip = ZipWriter::new(buf);
                let zip = perform_raw_copy_file(src, zip).unwrap();
                let out = zip.finish().unwrap().into_inner();

                assert_eq!(out.len(), len2);

                black_box(out)
            },
            BatchSize::SmallInput,
        );
    });

    // --- read_all_entries: iterate by_index and read each entry ---
    let bytes_all_entries =
        generate_random_archive_meta(read_all_entries_count(), 512).unwrap();
    c.bench_function("read_all_entries", |b| {
        b.iter(|| {
            let mut archive = ZipArchive::new(Cursor::new(bytes_all_entries.as_slice())).unwrap();
            for i in 0..archive.len() {
                let mut entry = archive.by_index(i).unwrap();
                let _ = io::copy(&mut entry, &mut io::sink()).unwrap();
            }
        });
    });

    // --- by_name_lookup_many: look up many names in large archive ---
    let lookup_names: Vec<String> = (0..by_name_lookup_count())
        .map(|i| format!("file_deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef_{i}.dat"))
        .collect();
    c.bench_function("by_name_lookup_many", |b| {
        b.iter(|| {
            let mut archive = ZipArchive::new(Cursor::new(bytes_meta.as_slice())).unwrap();
            for name in &lookup_names {
                let _ = archive.by_name(name).unwrap();
            }
        });
    });

    // --- parse_archive_with_comment ---
    let bytes_comment = generate_archive_with_comment(comment_size()).unwrap();
    c.bench_function("parse_archive_with_comment", |b| {
        b.iter(|| {
            let archive = ZipArchive::new(Cursor::new(bytes_comment.as_slice())).unwrap();
            black_box(archive.comment().len());
        });
    });

    // --- read_stream_entries: read_zipfile_from_stream until None ---
    let bytes_stream = generate_random_archive_meta(STREAM_ENTRIES, STREAM_ENTRY_SIZE).unwrap();
    let dir_stream = tempfile::TempDir::with_prefix("criterion_stream").unwrap();
    let path_stream = dir_stream.path().join("bench.zip");
    fs::write(&path_stream, &bytes_stream).unwrap();
    c.bench_function("read_stream_entries", |b| {
        b.iter(|| {
            let mut f = fs::File::open(&path_stream).unwrap();
            while zip::read::read_zipfile_from_stream(&mut f).unwrap().is_some() {}
        });
    });

    // --- parse_large_non_zip_reject ---
    let dir_reject = tempfile::TempDir::with_prefix("criterion_reject").unwrap();
    let path_reject = dir_reject.path().join("zeros");
    fs::write(&path_reject, vec![0u8; large_non_zip_size()]).unwrap();
    c.bench_function("parse_large_non_zip_reject", |b| {
        b.iter(|| {
            let r = ZipArchive::new(fs::File::open(&path_reject).unwrap());
            assert!(r.is_err());
        });
    });

    // --- write_many_small_files ---
    let payload_small = seeded_random_bytes(128);
    c.bench_function("write_many_small_files", |b| {
        b.iter(|| {
            let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
            let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            for i in 0..write_many_count() {
                let name = format!("file_{i}.dat");
                writer.start_file(name, options).unwrap();
                writer.write_all(&payload_small).unwrap();
            }
            black_box(writer.finish().unwrap().into_inner());
        });
    });

    // --- write_one_large_file ---
    let payload_large = seeded_random_bytes(WRITE_LARGE_SIZE);
    c.bench_function("write_one_large_file", |b| {
        b.iter(|| {
            let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
            let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            writer.start_file("large.dat", options).unwrap();
            writer.write_all(&payload_large).unwrap();
            black_box(writer.finish().unwrap().into_inner());
        });
    });

    // --- write_then_read_roundtrip ---
    let roundtrip_payload = seeded_random_bytes(256);
    c.bench_function("write_then_read_roundtrip", |b| {
        b.iter(|| {
            let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
            let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            for i in 0..ROUNDTRIP_ENTRIES {
                writer.start_file(format!("entry_{i}.dat"), options).unwrap();
                writer.write_all(&roundtrip_payload).unwrap();
            }
            let bytes = writer.finish().unwrap().into_inner();
            let archive = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
            black_box(archive.len());
        });
    });

    // --- deflate: read deflated entry (when feature enabled) ---
    #[cfg(feature = "deflate")]
    {
        let (_, deflated_bytes) = generate_random_archive_merge(
            5,
            4096,
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
            .unwrap();
        c.bench_function("read_deflated_entry", |b| {
            b.iter(|| {
                let mut archive = ZipArchive::new(Cursor::new(deflated_bytes.as_slice())).unwrap();
                for i in 0..archive.len() {
                    let mut entry = archive.by_index(i).unwrap();
                    let _ = io::copy(&mut entry, &mut io::sink()).unwrap();
                }
            });
        });
    }

    #[cfg(feature = "deflate")]
    {
        let deflate_payload = seeded_random_bytes(2048);
        c.bench_function("write_deflated_entries", |b| {
            b.iter(|| {
                let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
                let options =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
                for i in 0..20 {
                    writer.start_file(format!("deflated_{i}.dat"), options).unwrap();
                    writer.write_all(&deflate_payload).unwrap();
                }
                black_box(writer.finish().unwrap().into_inner());
            });
        });
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);