#![no_main]

use libfuzzer_sys::fuzz_target;
use tikv_jemallocator::Jemalloc;

use std::io::prelude::*;

use zip::read::read_zipfile_from_stream;
use zip::unstable::read::streaming::StreamingArchive;

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
    reader.rewind()?;
    while let Ok(Some(mut file)) = read_zipfile_from_stream(&mut reader) {
        std::io::copy(&mut file, &mut std::io::sink())?;
    }
    Ok(())
}

fn decompress_generic(data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let reader = std::io::Cursor::new(data);
    let mut zip = zip::ZipArchive::new(reader)?;

    for i in 0..zip.len() {
        let mut file = zip.by_index_generic(i)?.take(MAX_BYTES_TO_READ);
        std::io::copy(&mut file, &mut std::io::sink())?;
    }

    let mut reader = zip.into_inner();
    reader.rewind()?;
    let mut stream_zip = StreamingArchive::new(reader);

    while let Some(mut file) = stream_zip.next_entry()? {
        std::io::copy(&mut file, &mut std::io::sink())?;
    }
    while let Some(_) = stream_zip.next_metadata_entry()? {}
    Ok(())
}

fuzz_target!(|data: &[u8]| {
    let _ = decompress_all(data);
    let _ = decompress_generic(data);
});
