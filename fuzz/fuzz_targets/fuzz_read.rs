#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::{Read, Seek, SeekFrom};
use tikv_jemallocator::Jemalloc;
use zip::unstable::{
    read::streaming::{StreamingArchive, StreamingZipEntry, ZipStreamFileMetadata},
    stream::ZipStreamVisitor,
};

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

    struct V;
    impl ZipStreamVisitor for V {
        fn visit_file(&mut self, file: &mut StreamingZipEntry<impl Read>) -> ZipResult<()> {
            std::io::copy(&mut file, &mut std::io::sink())?
        }

        fn visit_additional_metadata(&mut self, metadata: &ZipStreamFileMetadata) -> ZipResult<()> {
            Ok(())
        }
    }

    let archive = StreamingArchive::new(reader)?;
    archive.visit(&mut V)?;

    Ok(())
}

fuzz_target!(|data: &[u8]| {
    let _ = decompress_all(data);
});
