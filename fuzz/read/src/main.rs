#![allow(unexpected_cfgs)] // Needed for cfg(fuzzing) on nightly as of 2024-05-06

#[cfg(fuzzing)]
use afl::fuzz;
use std::io::{Read, Seek, SeekFrom};
#[cfg(fuzzing)]
use tikv_jemallocator::Jemalloc;
use zip::read::read_zipfile_from_stream;

const MAX_BYTES_TO_READ: u64 = 1 << 24;

#[cfg(fuzzing)]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

fn decompress_all(reader: impl Read + Seek) -> Result<(), Box<dyn std::error::Error>> {
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

fn main() {
    #[cfg(fuzzing)]
    fuzz!(|data: &[u8]| {
        let reader = std::io::Cursor::new(data);
        let _ = decompress_all(reader);
    });

    #[cfg(not(fuzzing))]
    {
        let mut v = Vec::new();
        std::io::stdin().read_to_end(&mut v).unwrap();
        let reader = std::io::Cursor::new(v);
        decompress_all(reader).unwrap();
    }
}
