#[cfg(feature = "_deflate-any")]
fn bench_many_small_files(c: &mut criterion::Criterion) {
    use criterion::{BenchmarkId, Throughput};
    use std::io::{Cursor, Write};
    use zip::CompressionMethod;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let mut group = c.benchmark_group("deflate_many_small");
    for n in [10, 100, 500].iter() {
        let data = vec![b'x'; 1024];
        group.throughput(Throughput::Bytes((*n as u64) * 1024));
        group.bench_with_input(BenchmarkId::new("deflate", n), n, |b, &n| {
            b.iter(|| {
                let buf = Cursor::new(Vec::new());
                let mut zip = ZipWriter::new(buf);
                let opts =
                    SimpleFileOptions::default().compression_method(CompressionMethod::DEFLATE);
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
                let opts =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
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

#[cfg(feature = "_deflate-any")]
criterion::criterion_group!(benches, bench_many_small_files);
#[cfg(feature = "_deflate-any")]
criterion::criterion_main!(benches);
