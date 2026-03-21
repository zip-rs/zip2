use bencher::{benchmark_group, benchmark_main};

use std::io::{Cursor, Write};

use bencher::Bencher;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

const NB_FILES: usize = 100;
const FILENAME: &str = "bench_file_listing.zip";

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
    use std::fs::File;

    let bytes = generate_random_archive(size)?;
    let mut file = File::create(FILENAME)?;
    file.write_all(&bytes)?;
    Ok(())
}

fn file_listing_memory(bench: &mut Bencher) {
    let size = 1024 * 1024;
    let bytes = generate_random_archive(size)
        .expect("Failed to create a random archive for the bench read_entry()");

    bench.iter(|| {
        let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let mut names = vec![];
        for idx in 0..archive.len() {
            let file = archive.by_index(idx).unwrap();
            names.push(file.name().to_string());
        }
    });
}

fn file_listing_file(bench: &mut Bencher) {
    use std::fs::File;

    let size = 1024 * 1024;
    generate_random_archive_to_file(size)
        .expect("Failed to create a random archive for the bench read_entry()");

    bench.iter(|| {
        let file = File::open(FILENAME).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let mut names = vec![];
        for idx in 0..archive.len() {
            let file = archive.by_index(idx).unwrap();
            names.push(file.name().to_string());
        }
    });

    std::fs::remove_file(FILENAME).unwrap();
}

fn file_listing_iterable_memory(bench: &mut Bencher) {
    use zip::read::Config;
    use zip::unstable::read::IterableZip;
    let size = 1024 * 1024;
    let bytes = generate_random_archive(size)
        .expect("Failed to create a random archive for the bench read_entry()");

    bench.iter(|| {
        let mut archive = IterableZip::try_new(Cursor::new(&bytes), Config::default()).unwrap();
        let mut names = vec![];
        for file in archive.files().unwrap() {
            let file = file.unwrap();
            names.push(file.file_name().unwrap().to_string());
        }
    });
}

fn file_listing_iterable_file(bench: &mut Bencher) {
    use std::fs::File;
    use zip::read::Config;
    use zip::unstable::read::IterableZip;

    let size = 1024 * 1024;
    generate_random_archive_to_file(size)
        .expect("Failed to create a random archive for the bench read_entry()");

    bench.iter(|| {
        let file = File::open(FILENAME).unwrap();
        let mut archive = IterableZip::try_new(file, Config::default()).unwrap();
        let mut names = vec![];
        for file in archive.files().unwrap() {
            let file = file.unwrap();
            names.push(file.file_name().unwrap().to_string());
        }
    });
    std::fs::remove_file(FILENAME).unwrap();
}

benchmark_group!(
    benches,
    file_listing_memory,
    file_listing_iterable_memory,
    file_listing_file,
    file_listing_iterable_file
);
benchmark_main!(benches);
