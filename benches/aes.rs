//! Criterion benchmarks for AES-256 encrypted entries.
//!
//! Usage:
//!
//! ```sh
//! cargo bench --bench aes --features aes-crypto
//! ```
//!
//! One Stored entry per archive, so no compression time is included. Each iteration also
//! derives the key with PBKDF2 once.

#[cfg(not(any(miri, target_arch = "wasm32")))]
mod benches {
    use criterion::{BenchmarkId, Criterion, Throughput, criterion_group};
    use std::hint::black_box;
    use std::io::{Cursor, Read, Write};
    use zip::{AesMode, CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

    const ENTRY_SIZES: &[usize] = &[64 * 1024, 8 * 1024 * 1024];
    const WRITE_CHUNK: usize = 64 * 1024;
    const PASSWORD: &str = "password";

    fn encrypt(data: &[u8]) -> Vec<u8> {
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .with_aes_encryption(AesMode::Aes256, PASSWORD);
        let mut writer = ZipWriter::new(Cursor::new(Vec::with_capacity(data.len() + 1024)));
        writer.start_file("data.bin", options).unwrap();
        for chunk in data.chunks(WRITE_CHUNK) {
            writer.write_all(chunk).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn criterion_benchmark(c: &mut Criterion) {
        let mut group = c.benchmark_group("aes256_stored");
        for &size in ENTRY_SIZES {
            // AES throughput doesn't depend on the content.
            let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let archive = encrypt(&data);
            group.throughput(Throughput::Bytes(size as u64));
            group.bench_with_input(BenchmarkId::new("encrypt", size), &data, |b, data| {
                b.iter(|| black_box(encrypt(data)));
            });
            group.bench_with_input(BenchmarkId::new("decrypt", size), &archive, |b, archive| {
                let mut out = Vec::with_capacity(size);
                b.iter(|| {
                    let mut zip = ZipArchive::new(Cursor::new(archive.as_slice())).unwrap();
                    let mut file = zip.by_index_decrypt(0, PASSWORD.as_bytes()).unwrap();
                    out.clear();
                    file.read_to_end(&mut out).unwrap();
                    black_box(&out);
                });
            });
        }
        group.finish();
    }

    criterion_group!(benches, criterion_benchmark);
}
#[cfg(not(any(miri, target_arch = "wasm32")))]
use benches::benches;
#[cfg(not(any(miri, target_arch = "wasm32")))]
criterion::criterion_main!(benches);
#[cfg(any(miri, target_arch = "wasm32"))]
fn main() {
    eprintln!("aes cannot run on miri or wasm32 targets; skipping it");
}
