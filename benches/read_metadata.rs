use bencher::{benchmark_group, benchmark_main};

use std::io::{Cursor, Write};

use bencher::Bencher;
use getrandom::getrandom;
use zip::write::SimpleFileOptions;
use zip::{result::ZipResult, CompressionMethod, ZipArchive, ZipWriter};

const FILE_COUNT: usize = 15_000;
const FILE_SIZE: usize = 1024;

fn generate_random_archive(count_files: usize, file_size: usize) -> ZipResult<Vec<u8>> {
    let data = Vec::new();
    let mut writer = ZipWriter::new(Cursor::new(data));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let mut bytes = vec![0u8; file_size];

    for i in 0..count_files {
        let name = format!("file_deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef_{i}.dat");
        writer.start_file(name, options)?;
        getrandom(&mut bytes).unwrap();
        writer.write_all(&bytes)?;
    }

    Ok(writer.finish()?.into_inner())
}

fn read_metadata(bench: &mut Bencher) {
    let bytes = generate_random_archive(FILE_COUNT, FILE_SIZE).unwrap();

    bench.iter(|| {
        let archive = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        archive.len()
    });
}

const COMMENT_SIZE: usize = 50_000;

fn generate_random_zip32_archive_with_comment(comment_length: usize) -> ZipResult<Vec<u8>> {
    let data = Vec::new();
    let mut writer = ZipWriter::new(Cursor::new(data));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let mut bytes = vec![0u8; comment_length];
    getrandom(&mut bytes).unwrap();
    writer.set_raw_comment(bytes);

    writer.start_file("asdf.txt", options)?;
    writer.write_all(b"asdf")?;

    Ok(writer.finish()?.into_inner())
}

fn parse_comment(bench: &mut Bencher) {
    let bytes = generate_random_zip32_archive_with_comment(COMMENT_SIZE).unwrap();

    bench.iter(|| {
        let archive = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        archive.len()
    });
}

const COMMENT_SIZE_64: usize = 500_000;

fn generate_random_zip64_archive_with_comment(comment_length: usize) -> ZipResult<Vec<u8>> {
    let data = Vec::new();
    let mut writer = ZipWriter::new(Cursor::new(data));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(true);

    let mut bytes = vec![0u8; comment_length];
    getrandom(&mut bytes).unwrap();
    writer.set_raw_comment(bytes);

    writer.start_file("asdf.txt", options)?;
    writer.write_all(b"asdf")?;

    Ok(writer.finish()?.into_inner())
}

fn parse_zip64_comment(bench: &mut Bencher) {
    let bytes = generate_random_zip64_archive_with_comment(COMMENT_SIZE_64).unwrap();

    bench.iter(|| {
        let archive = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        archive.len()
    });
}

benchmark_group!(benches, read_metadata, parse_comment, parse_zip64_comment);
benchmark_main!(benches);
