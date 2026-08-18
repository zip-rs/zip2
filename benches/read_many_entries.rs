use bencher::{benchmark_group, benchmark_main};

use std::fs::File;
use std::io::{Cursor, Write};

use bencher::Bencher;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

const NB_FILES: usize = 100;
const FILENAME: &str = "bench_read_many_entries.zip";

fn generate_random_archive(size: usize) -> Result<Vec<u8>, std::io::Error> {
    let data = Vec::new();
    let mut writer = ZipWriter::new(Cursor::new(data));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for count in 0..NB_FILES {
        writer.start_file(format!("random_{}.dat", count), options)?;
        let mut bytes = vec![0u8; size];
        getrandom::fill(&mut bytes)
            .map_err(|e| std::io::Error::other(format!("getrandom error: {}", e)))?;
        writer.write_all(&bytes)?;
    }
    let w = writer.finish()?;

    Ok(w.into_inner())
}

fn generate_random_archive_to_file(size: usize) -> Result<(), std::io::Error> {
    let bytes = generate_random_archive(size)?;
    let mut file = File::create(FILENAME)?;
    file.write_all(&bytes)?;
    Ok(())
}

fn read_many_entries_as_file(bench: &mut Bencher) {
    let size = 1024 * 1024;
    generate_random_archive_to_file(size)
        .expect("Failed to create a random archive for the bench read_entry()");

    bench.iter(|| {
        let file = File::open(FILENAME).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        for idx in 0..archive.len() {
            let mut file = archive.by_index(idx).unwrap();
            let _n = std::io::copy(&mut file, &mut std::io::sink()).unwrap();
        }
    });

    bench.bytes = (size * NB_FILES) as u64;
    std::fs::remove_file(FILENAME).unwrap();
}

fn read_many_entries_as_file_buf(bench: &mut Bencher) {
    let size = 1024 * 1024;
    generate_random_archive_to_file(size)
        .expect("Failed to create a random archive for the bench read_entry()");

    bench.iter(|| {
        let file = File::open(FILENAME).unwrap();
        let file = std::io::BufReader::new(file);
        let mut archive = ZipArchive::new(file).unwrap();
        for idx in 0..archive.len() {
            let mut file = archive.by_index(idx).unwrap();
            let _n = std::io::copy(&mut file, &mut std::io::sink()).unwrap();
        }
    });

    bench.bytes = (size * NB_FILES) as u64;
    std::fs::remove_file(FILENAME).unwrap();
}

fn read_many_entries(bench: &mut Bencher) {
    let size = 1024 * 1024;
    let bytes = generate_random_archive(size)
        .expect("Failed to create a random archive for the bench read_entry()");

    bench.iter(|| {
        let mut archive = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
        for idx in 0..archive.len() {
            let mut file = archive.by_index(idx).unwrap();
            let _n = std::io::copy(&mut file, &mut std::io::sink()).unwrap();
        }
    });

    bench.bytes = (size * NB_FILES) as u64;
}

benchmark_group!(
    benches,
    read_many_entries,
    read_many_entries_as_file,
    read_many_entries_as_file_buf
);
benchmark_main!(benches);
