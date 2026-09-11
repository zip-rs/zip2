use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use std::io::{Cursor, Write};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;
use zip::CompressionMethod;

fn bench_many_small_files(c: &mut Criterion) {
    let mut group = c.benchmark_group("deflate_many_small");
    for n in [10, 100, 500].iter() {
        let data = vec![b'x'; 1024];
        group.throughput(Throughput::Bytes((*n as u64)*1024));
        group.bench_with_input(BenchmarkId::new("deflate", n), n, |b, &n| {
            b.iter(|| {
                let buf = Cursor::new(Vec::new());
                let mut zip = ZipWriter::new(buf);
                let opts = SimpleFileOptions::default().compression_method(CompressionMethod::DEFLATE);
                for i in 0..n {
                    zip.start_file(format!("file_{:04}.txt", i), opts).unwrap();
                    zip.write_all(&data).unwrap();
                }
                zip.finish().unwrap()
            })
        });
        group.bench_with_input(BenchmarkId::new("stored", n), n, |b, &n| {
            b.iter(|| {
                let buf = Cursor::new(Vec::new());
                let mut zip = ZipWriter::new(buf);
                let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
                for i in 0..n {
                    zip.start_file(format!("file_{:04}.txt", i), opts).unwrap();
                    zip.write_all(&data).unwrap();
                }
                zip.finish().unwrap()
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_many_small_files);
criterion_main!(benches);
