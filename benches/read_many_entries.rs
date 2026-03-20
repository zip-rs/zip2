use bencher::{benchmark_group, benchmark_main};

use std::io::{Cursor, Read, Write};

use bencher::Bencher;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

const NB_FILES: usize = 200;

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

fn read_many_entries(bench: &mut Bencher) {
    let size = 1024 * 1024;
    let bytes = generate_random_archive(size)
        .expect("Failed to create a random archive for the bench read_entry()");
    let mut archive = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();

    bench.iter(|| {
        for idx in 0..archive.len() {
            let mut file = archive.by_index(idx).unwrap();
            let mut buf = [0u8; 1024];
            loop {
                let n = file.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
            }
        }
    });

    bench.bytes = (size * NB_FILES) as u64;
}

benchmark_group!(benches, read_many_entries);
benchmark_main!(benches);
