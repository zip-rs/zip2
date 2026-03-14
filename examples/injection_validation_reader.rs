//! Read a ZIP archive with CRC checking disabled by using a custom final reader.
//! The built-in final reader (Crc32Reader) is replaced with a pass-through that does not validate CRC.

use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use zip::read::Config;
use zip::{ValidatorReaderFactory, ZipArchive, ZipReadOptions};

/// Final reader that just passes through the decompressed stream (no CRC validation).
fn skip_crc_check<'a>(
    inner: Box<dyn Read + 'a>,
    _crc32: u32,
    _ae2: bool,
) -> Box<dyn Read + 'a> {
    inner
}

fn main() -> zip::result::ZipResult<()> {
    // Override with your file; make sure to use a small dummy zip
    let file = File::open("/home/user/dummy.zip")?;
    let config = Config::default();
    let mut archive = ZipArchive::with_config(config, file)?;

    let factory: Arc<ValidatorReaderFactory> = Arc::new(skip_crc_check);

    for i in 0..archive.len() {
        let options = ZipReadOptions::new().final_reader_factory(Some(Arc::clone(&factory)));
        let mut zip_file = archive.by_index_with_options(i, options)?;
        println!("Filename: {}", zip_file.name());
        println!("---");
        let mut buf = Vec::new();
        zip_file.read_to_end(&mut buf)?;
        let content = String::from_utf8_lossy(&buf);
        print!("{content}");
        println!("---");
    }

    Ok(())
}
