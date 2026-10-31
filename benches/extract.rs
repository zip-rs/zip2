//! Criterion benchmarks for sequential vs parallel archive extraction.
//!
//! Usage:
//!
//! ```sh
//! cargo bench --bench extract --features rayon
//! ```

#[cfg(feature = "rayon")]
fn bench_extract_parallel_vs_sequential(c: &mut criterion::Criterion) {
    use criterion::{BenchmarkId, Throughput};
    use std::io::{Cursor, Write};
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    #[cfg(not(miri))]
    const FILES_PER_ARCHIVE: &[usize] = &[20, 50];

    #[cfg(miri)]
    const FILES_PER_ARCHIVE: &[usize] = &[5];

    let mut group = c.benchmark_group("extract");
    for &n in FILES_PER_ARCHIVE {
        let file_size: usize = 64 * 1024; // 64 KiB per entry
        let payload = vec![b'a'; file_size];
        let filenames: Vec<String> = (0..n).map(|i| format!("entry_{:04}.dat", i)).collect();

        let mut zip_writer = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for name in &filenames {
            zip_writer.start_file(name, opts).unwrap();
            zip_writer.write_all(&payload).unwrap();
        }
        let archive_bytes = zip_writer.finish().unwrap().into_inner();
        let total_bytes = (n as u64) * (file_size as u64);

        group.throughput(Throughput::Bytes(total_bytes));

        group.bench_with_input(BenchmarkId::new("sequential", n), &n, |b, _| {
            b.iter(|| {
                let temp_dir = TempDir::with_prefix("bench_seq_extract").unwrap();
                let mut archive = ZipArchive::new(Cursor::new(archive_bytes.as_slice())).unwrap();
                archive.extract(temp_dir.path()).unwrap();
            });
        });

        group.bench_with_input(BenchmarkId::new("parallel", n), &n, |b, _| {
            b.iter(|| {
                let temp_dir = TempDir::with_prefix("bench_par_extract").unwrap();
                let mut archive = ZipArchive::new(Cursor::new(archive_bytes.as_slice())).unwrap();
                archive.par_extract(temp_dir.path()).unwrap();
            });
        });
    }
    group.finish();
}

#[cfg(feature = "rayon")]
criterion::criterion_group!(benches, bench_extract_parallel_vs_sequential);
#[cfg(feature = "rayon")]
criterion::criterion_main!(benches);

#[cfg(not(feature = "rayon"))]
fn main() {}
