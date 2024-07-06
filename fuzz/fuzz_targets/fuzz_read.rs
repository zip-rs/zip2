#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::{Read, Seek, SeekFrom};
use tikv_jemallocator::Jemalloc;
use zip::read::read_zipfile_from_stream;

const MAX_BYTES_TO_READ: u64 = 1 << 24;

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

fn decompress_all(data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let reader = std::io::Cursor::new(data);
    let mut zip = zip::ZipArchive::new(reader)?;

    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?.take(MAX_BYTES_TO_READ);
        std::io::copy(&mut file, &mut std::io::sink())?;
    }
    let mut reader = zip.into_inner();
    reader.seek(SeekFrom::Start(0))?;
    while let Ok(Some(mut file)) = read_zipfile_from_stream(&mut reader) {
        std::io::copy(&mut file, &mut std::io::sink())?;
    }
    Ok(())
}

fuzz_target!(|data: &[u8]| {
    let _ = decompress_all(data);
});
