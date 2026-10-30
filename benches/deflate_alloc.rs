#[cfg(feature = "_deflate-any")]
fn bench_many_small_files(c: &mut criterion::Criterion) {
    use criterion::{BenchmarkId, Throughput};
    use std::io::{Cursor, Write};
    use zip::CompressionMethod;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    #[cfg(not(miri))]
    const FILES_PER_ARCHIVE: &[usize] = &[10, 100, 500];

    #[cfg(miri)]
    const FILES_PER_ARCHIVE: &[usize] = &[10];

    let mut group = c.benchmark_group("deflate_many_small");
    for n in FILES_PER_ARCHIVE.iter() {
        let data = vec![b'x'; 1024];
        let filenames: Vec<String> = (0..*n).map(|i| format!("file_{:04}.txt", i)).collect();
        group.throughput(Throughput::Bytes((*n as u64) * 1024));
        group.bench_with_input(BenchmarkId::new("deflate", n), n, |b, &_n| {
            b.iter(|| {
                let buf = Cursor::new(Vec::new());
                let mut zip = ZipWriter::new(buf);
                let opts =
                    SimpleFileOptions::default().compression_method(CompressionMethod::DEFLATE);
                for filename in &filenames {
                    zip.start_file(filename, opts).unwrap();
                    zip.write_all(&data).unwrap();
                }
                zip.finish().unwrap()
            })
        });
        #[cfg(feature = "rayon")]
        group.bench_with_input(
            BenchmarkId::new("deflate_parallel_rayon", n),
            n,
            |b, &_n| {
                let opts =
                    SimpleFileOptions::default().compression_method(CompressionMethod::DEFLATE);
                let entries: Vec<(&str, SimpleFileOptions, &[u8])> = filenames
                    .iter()
                    .map(|f| (f.as_str(), opts, data.as_slice()))
                    .collect();
                b.iter(|| {
                    let buf = Cursor::new(Vec::new());
                    let mut zip = ZipWriter::new(buf);
                    zip.par_append_data(entries.clone()).unwrap();
                    zip.finish().unwrap()
                })
            },
        );
        group.bench_with_input(BenchmarkId::new("stored", n), n, |b, &_n| {
            b.iter(|| {
                let buf = Cursor::new(Vec::new());
                let mut zip = ZipWriter::new(buf);
                let opts =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
                for filename in &filenames {
                    zip.start_file(filename, opts).unwrap();
                    zip.write_all(&data).unwrap();
                }
                zip.finish().unwrap()
            })
        });
    }
    group.finish();
}

#[cfg(all(feature = "_deflate-any", feature = "rayon"))]
fn bench_parallel_deflate(c: &mut criterion::Criterion) {
    use criterion::{BenchmarkId, Throughput};
    use std::io::{Cursor, Write};
    use std::num::NonZeroUsize;
    use zip::CompressionMethod;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    // Realistic compressible payload: 64 KiB structured text
    let chunk = b"Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. ";
    let mut payload_64k = Vec::with_capacity(64 * 1024);
    while payload_64k.len() + chunk.len() <= 64 * 1024 {
        payload_64k.extend_from_slice(chunk);
    }

    #[cfg(not(miri))]
    const FILE_COUNTS: &[usize] = &[20, 100];
    #[cfg(miri)]
    const FILE_COUNTS: &[usize] = &[5];

    let mut group = c.benchmark_group("deflate_parallel_throughput");

    for &n in FILE_COUNTS {
        let total_bytes = (n as u64) * (payload_64k.len() as u64);
        group.throughput(Throughput::Bytes(total_bytes));
        let filenames: Vec<String> = (0..n).map(|i| format!("doc_{:04}.txt", i)).collect();
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::DEFLATE);

        // Sequential ZipWriter
        group.bench_with_input(BenchmarkId::new("sequential", n), &n, |b, &_n| {
            b.iter(|| {
                let buf = Cursor::new(Vec::new());
                let mut zip = ZipWriter::new(buf);
                for filename in &filenames {
                    zip.start_file(filename, opts).unwrap();
                    zip.write_all(&payload_64k).unwrap();
                }
                zip.finish().unwrap()
            })
        });

        // Parallel ZipWriter (par_append_data)
        let entries: Vec<(&str, SimpleFileOptions, &[u8])> = filenames
            .iter()
            .map(|f| (f.as_str(), opts, payload_64k.as_slice()))
            .collect();
        group.bench_with_input(BenchmarkId::new("parallel_rayon", n), &n, |b, &_n| {
            b.iter(|| {
                let buf = Cursor::new(Vec::new());
                let mut zip = ZipWriter::new(buf);
                zip.par_append_data(entries.clone()).unwrap();
                zip.finish().unwrap()
            })
        });

        // Batched Parallel ZipWriter (par_append_data_batches, batch_size = 25)
        group.bench_with_input(
            BenchmarkId::new("parallel_rayon_batches_25", n),
            &n,
            |b, &_n| {
                b.iter(|| {
                    let buf = Cursor::new(Vec::new());
                    let mut zip = ZipWriter::new(buf);
                    zip.par_append_data_batches(&entries, NonZeroUsize::new(25).unwrap())
                        .unwrap();
                    zip.finish().unwrap()
                })
            },
        );
    }
    group.finish();
}

#[cfg(all(feature = "_deflate-any", feature = "rayon"))]
criterion::criterion_group!(benches, bench_many_small_files, bench_parallel_deflate);
#[cfg(all(feature = "_deflate-any", not(feature = "rayon")))]
criterion::criterion_group!(benches, bench_many_small_files);
#[cfg(feature = "_deflate-any")]
criterion::criterion_main!(benches);
