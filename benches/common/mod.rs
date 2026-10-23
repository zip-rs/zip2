use std::io::{Cursor, Read, Seek, Write};
use zip::result::ZipResult;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

// Deterministic seeded randomness helper (SplitMix64; no external dependencies, no syscalls).
// Why we use this instead of the getrandom crate:
// 1. Deterministic — always the same bytes.
// 2. No syscalls — the getrandom crate calls the OS/platform RNG.
// 3. No dependency here — getrandom is used elsewhere but remains optional and may change.
pub fn seeded_random_bytes(size: usize) -> Box<[u8]> {
    let mut x: u64 = 0xdead_beef_cafe_babe;
    let mut out = vec![0u8; size].into_boxed_slice();

    for chunk in out.chunks_mut(8) {
        x = x.wrapping_add(0x9E3779B97F4A7C15);

        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;

        let bytes = z.to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }

    out
}

pub fn generate_random_archive(size: usize) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.start_file("random.dat", options).unwrap();
    writer.write_all(&seeded_random_bytes(size)).unwrap();
    writer.finish().unwrap().into_inner()
}

pub fn generate_random_archive_meta(count_files: usize, file_size: usize) -> ZipResult<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let bytes = seeded_random_bytes(file_size);

    for i in 0..count_files {
        let name = format!("file_deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef_{i}.dat");
        writer.start_file(name, options)?;
        writer.write_all(&bytes)?;
    }

    Ok(writer.finish()?.into_inner())
}

pub fn generate_random_archive_merge(
    num_entries: usize,
    entry_size: usize,
    options: SimpleFileOptions,
) -> ZipResult<(usize, Vec<u8>)> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let bytes = seeded_random_bytes(entry_size * num_entries);

    for (i, file) in bytes.chunks_exact(entry_size).enumerate() {
        writer.start_file(format!("random{i}.dat"), options)?;
        writer.write_all(file)?;
    }

    let bytes = writer.finish()?.into_inner();
    Ok((bytes.len(), bytes))
}

pub fn perform_raw_copy_file_by_index<R: Read + Seek, W: Write + Seek>(
    mut src: ZipArchive<R>,
    mut target: ZipWriter<W>,
) -> ZipResult<ZipWriter<W>> {
    for i in 0..src.len() {
        let entry = src.by_index_raw(i)?;
        target.raw_copy_file(entry)?;
    }
    Ok(target)
}

pub fn generate_archive_with_comment(comment_len: usize) -> ZipResult<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.set_raw_comment(seeded_random_bytes(comment_len))?;
    writer.start_file("data.txt", options)?;
    writer.write_all(b"x")?;
    Ok(writer.finish()?.into_inner())
}
