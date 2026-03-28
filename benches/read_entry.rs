use bencher::{benchmark_group, benchmark_main};

use std::io::{Cursor, Read, Write};

use bencher::Bencher;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

fn generate_random_archive(size: usize) -> Result<Vec<u8>, std::io::Error> {
    let data = Vec::new();
    let mut writer = ZipWriter::new(Cursor::new(data));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    writer.start_file("random.dat", options)?;
    let mut bytes = vec![0u8; size];
    getrandom::fill(&mut bytes)
        .map_err(|e| std::io::Error::other(format!("getrandom error: {}", e)))?;
    writer.write_all(&bytes)?;

    let w = writer.finish()?;

    Ok(w.into_inner())
}

fn read_entry(bench: &mut Bencher) {
    let size = 1024 * 1024;
    let bytes = generate_random_archive(size)
        .expect("Failed to create a random archive for the bench read_entry()");
    let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();

    bench.iter(|| {
        let mut file = archive.by_name("random.dat").unwrap();
        let mut buf = [0u8; 1024];
        loop {
            let n = file.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
        }
    });

    bench.bytes = size as u64;
}

fn read_entry_iterable(bench: &mut Bencher) {
    use zip::read::Config;
    use zip::unstable::read::IterableZip;
    let size = 1024 * 1024;
    let bytes = generate_random_archive(size)
        .expect("Failed to create a random archive for the bench read_entry()");
    let mut reader = Cursor::new(&bytes);
    let mut archive = IterableZip::try_new(reader.clone(), Config::default()).unwrap();

    bench.iter(|| {
        let file = archive
            .files()
            .unwrap()
            .find(|f| {
                let file = f.as_ref().unwrap();
                let filename = file.file_name().unwrap();
                filename == "random.dat"
            })
            .unwrap()
            .unwrap();
        let mut buf = [0u8; 1024];
        let zip_data = &file.into_zip_file_data(&mut reader).unwrap();
        let mut file_reader = archive.by_file_data(&zip_data, Default::default()).unwrap();
        loop {
            let n = file_reader.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
        }
    });

    bench.bytes = size as u64;
}

benchmark_group!(benches, read_entry, read_entry_iterable);
benchmark_main!(benches);
