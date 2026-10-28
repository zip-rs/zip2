//! Benchmarks for listing ZIP entries with and without an in-memory index.
//!
//! Usage:
//!
//! ```sh
//! cargo bench --bench zip_file_list
//! ```

#[cfg(not(any(miri, target_arch = "wasm32")))]
mod benches {
    use criterion::{Criterion, criterion_group};
    use std::hint::black_box;
    use std::io::{Cursor, Write};
    use zip::CompressionMethod;
    use zip::ZipArchive;
    use zip::ZipWriter;
    use zip::read::Config;
    use zip::unstable::read::ZipIterable;
    use zip::write::SimpleFileOptions;

    const ENTRY_COUNT: usize = 5_000;
    const ENTRY_SIZE: usize = 64;

    fn build_archive() -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for index in 0..ENTRY_COUNT {
            writer
                .start_file(format!("dir/entry-{index:05}.txt"), options)
                .unwrap();
            writer.write_all(&[b'x'; ENTRY_SIZE]).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn list_zip_archive(c: &mut Criterion) {
        let bytes = build_archive();
        c.bench_function("list_zip_archive", |b| {
            b.iter(|| {
                let mut archive = ZipArchive::new(Cursor::new(black_box(&bytes))).unwrap();
                let mut count = 0usize;
                for index in 0..archive.len() {
                    let entry = archive.by_index_raw(index).unwrap();
                    black_box(entry.name().unwrap());
                    count += 1;
                }
                black_box(count);
            });
        });
    }

    fn list_zip_iterable(c: &mut Criterion) {
        let bytes = build_archive();
        c.bench_function("list_zip_iterable", |b| {
            b.iter(|| {
                let mut archive =
                    ZipIterable::try_new(Cursor::new(black_box(&bytes)), Config::default())
                        .unwrap();
                let mut count = 0usize;
                for entry in archive.files().unwrap() {
                    let entry = entry.unwrap();
                    black_box(entry.name().unwrap());
                    count += 1;
                }
                black_box(count);
            });
        });
    }

    criterion_group!(benches, list_zip_archive, list_zip_iterable);
}

#[cfg(not(any(miri, target_arch = "wasm32")))]
use benches::benches;

#[cfg(not(any(miri, target_arch = "wasm32")))]
criterion::criterion_main!(benches);

#[cfg(any(miri, target_arch = "wasm32"))]
fn main() {
    eprintln!("zip_file_list cannot run on miri or wasm32 targets; skipping it");
}
