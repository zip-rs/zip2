//! Criterion benchmarks for ZIP operations using in-memory buffers only.
//!
//! Usage:
//!
//! ```sh
//! cargo bench --bench memory
//! ```

mod common;

use common::{
    generate_random_archive, generate_random_archive_merge, generate_random_archive_meta,
    seeded_random_bytes,
};
use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::io::{self, Cursor, Read, Seek, Write};
use zip::{CompressionMethod, ZipArchive, ZipWriter, result::ZipResult, write::SimpleFileOptions};

fn perform_merge<R: Read + Seek, W: Write + Seek>(
    src: ZipArchive<R>,
    mut target: ZipWriter<W>,
) -> ZipResult<ZipWriter<W>> {
    target.merge_archive(src)?;
    Ok(target)
}

fn perform_raw_copy_file_by_index<R: Read + Seek, W: Write + Seek>(
    mut src: ZipArchive<R>,
    mut target: ZipWriter<W>,
) -> ZipResult<ZipWriter<W>> {
    for i in 0..src.len() {
        let entry = src.by_index_raw(i)?;
        target.raw_copy_file(entry)?;
    }
    Ok(target)
}

fn generate_archive_with_comment(comment_len: usize) -> ZipResult<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.set_raw_comment(seeded_random_bytes(comment_len))?;
    writer.start_file("data.txt", options)?;
    writer.write_all(b"x")?;
    Ok(writer.finish()?.into_inner())
}

fn generate_zip64_archive_with_extensible_data(data_len: usize) -> ZipResult<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(true);
    writer.set_raw_zip64_extensible_data_sector(seeded_random_bytes(data_len));
    writer.start_file("data.txt", options)?;
    writer.write_all(b"x")?;
    Ok(writer.finish()?.into_inner())
}

fn memory_benchmarks(c: &mut Criterion) {
    c.bench_function("read_entry_in_memory", |b| {
        let archive_bytes = generate_random_archive(1024 * 1024);
        b.iter(|| {
            let mut archive = ZipArchive::new(Cursor::new(archive_bytes.as_slice())).unwrap();
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

    #[cfg(not(miri))]
    {
        c.bench_function("read_metadata_in_memory", |b| {
            let archive_bytes = generate_random_archive_meta(15_000, 1024).unwrap();
            b.iter(|| {
                black_box(
                    ZipArchive::new(Cursor::new(archive_bytes.as_slice()))
                        .unwrap()
                        .len(),
                )
            });
        });

        let many_entries = generate_random_archive_meta(100, 1024 * 1024).unwrap();
        let mut many_entries_group = c.benchmark_group("read_many_entries_memory");
        many_entries_group.throughput(criterion::Throughput::Bytes(100 * 1024 * 1024));
        many_entries_group.bench_function("read_all", |b| {
            b.iter(|| {
                let mut archive = ZipArchive::new(Cursor::new(many_entries.as_slice())).unwrap();
                for idx in 0..archive.len() {
                    let mut entry = archive.by_index(idx).unwrap();
                    io::copy(&mut entry, &mut io::sink()).unwrap();
                }
            });
        });
        many_entries_group.finish();
    }

    let comment_archive = generate_archive_with_comment(50_000).unwrap();
    c.bench_function("parse_archive_with_comment_in_memory", |b| {
        b.iter(|| {
            let archive = ZipArchive::new(Cursor::new(comment_archive.as_slice())).unwrap();
            black_box(archive.comment().len());
        });
    });

    let zip64_archive = generate_zip64_archive_with_extensible_data(500_000).unwrap();
    c.bench_function("parse_zip64_archive_with_extensible_data", |b| {
        b.iter(|| {
            let archive = ZipArchive::new(Cursor::new(zip64_archive.as_slice())).unwrap();
            black_box(archive.raw_zip64_extensible_data_sector().unwrap());
        });
    });

    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let (stored_len, stored_archive) = generate_random_archive_merge(100, 1024, options).unwrap();
    c.bench_function("merge_archive_stored_in_memory", |b| {
        b.iter(|| {
            let src = ZipArchive::new(Cursor::new(stored_archive.as_slice())).unwrap();
            let zip = ZipWriter::new(Cursor::new(Vec::new()));
            let out = perform_merge(src, zip)
                .unwrap()
                .finish()
                .unwrap()
                .into_inner();
            assert_eq!(out.len(), stored_len);
            black_box(out);
        });
    });

    let (raw_copy_len, raw_copy_archive) =
        generate_random_archive_merge(100, 1024, options).unwrap();
    c.bench_function("merge_archive_raw_copy_file_stored_by_index", |b| {
        b.iter(|| {
            let src = ZipArchive::new(Cursor::new(raw_copy_archive.as_slice())).unwrap();
            let zip = ZipWriter::new(Cursor::new(Vec::new()));
            let out = perform_raw_copy_file_by_index(src, zip)
                .unwrap()
                .finish()
                .unwrap()
                .into_inner();
            assert_eq!(out.len(), raw_copy_len);
            black_box(out);
        });
    });

    #[cfg(feature = "_deflate-any")]
    {
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let (deflated_len, deflated_archive) =
            generate_random_archive_merge(100, 1024, options).unwrap();
        c.bench_function("merge_archive_compressed_in_memory", |b| {
            b.iter(|| {
                let src = ZipArchive::new(Cursor::new(deflated_archive.as_slice())).unwrap();
                let zip = ZipWriter::new(Cursor::new(Vec::new()));
                let out = perform_merge(src, zip)
                    .unwrap()
                    .finish()
                    .unwrap()
                    .into_inner();
                assert_eq!(out.len(), deflated_len);
                black_box(out);
            });
        });

        let (deflated_raw_len, deflated_raw_archive) =
            generate_random_archive_merge(100, 1024, options).unwrap();
        c.bench_function("merge_archive_raw_copy_file_compressed", |b| {
            b.iter(|| {
                let src = ZipArchive::new(Cursor::new(deflated_raw_archive.as_slice())).unwrap();
                let zip = ZipWriter::new(Cursor::new(Vec::new()));
                let out = perform_raw_copy_file_by_index(src, zip)
                    .unwrap()
                    .finish()
                    .unwrap()
                    .into_inner();
                assert_eq!(out.len(), deflated_raw_len);
                black_box(out);
            });
        });
    }
}

criterion_group!(benches, memory_benchmarks);
criterion_main!(benches);
