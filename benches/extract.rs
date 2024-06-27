use bencher::{benchmark_group, benchmark_main};

use bencher::Bencher;
use tempdir::TempDir;

use std::fs;
use std::path::Path;

use zip::result::ZipResult;
use zip::ZipArchive;

#[cfg(all(feature = "parallelism", unix))]
use zip::read::{split_extract, ExtractionParameters};

/* This archive has a set of entries repeated 20x:
 * - 200K random data, stored uncompressed (CompressionMethod::Stored)
 * - 246K text data (the project gutenberg html version of king lear)
 *   (CompressionMethod::Bzip2, compression level 1) (project gutenberg ebooks are public domain)
 *
 * The full archive file is 5.3MB.
 */
fn get_test_archive() -> ZipResult<ZipArchive<fs::File>> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/stored-and-compressed-text.zip");
    let file = fs::File::open(path)?;
    ZipArchive::new(file)
}

fn extract_basic(bench: &mut Bencher) {
    let mut readable_archive = get_test_archive().unwrap();
    let total_size: u64 = readable_archive
        .decompressed_size()
        .unwrap()
        .try_into()
        .unwrap();

    let parent = TempDir::new("zip-extract").unwrap();

    bench.bytes = total_size;
    bench.bench_n(1, |bench| {
        bench.iter(move || {
            let outdir = TempDir::new_in(parent.path(), "bench-subdir")
                .unwrap()
                .into_path();
            readable_archive.extract(outdir).unwrap();
        });
    });
}

#[cfg(all(feature = "parallelism", unix))]
const DECOMPRESSION_THREADS: usize = 8;

#[cfg(all(feature = "parallelism", unix))]
fn extract_split(bench: &mut Bencher) {
    let readable_archive = get_test_archive().unwrap();
    let total_size: u64 = readable_archive
        .decompressed_size()
        .unwrap()
        .try_into()
        .unwrap();

    let params = ExtractionParameters {
        decompression_threads: DECOMPRESSION_THREADS,
        ..Default::default()
    };

    let parent = TempDir::new("zip-extract").unwrap();

    bench.bytes = total_size;
    bench.bench_n(1, |bench| {
        bench.iter(move || {
            let outdir = TempDir::new_in(parent.path(), "bench-subdir")
                .unwrap()
                .into_path();
            split_extract(&readable_archive, &outdir, params.clone()).unwrap();
        });
    });
}

#[cfg(not(all(feature = "parallelism", unix)))]
benchmark_group!(benches, extract_basic);

#[cfg(all(feature = "parallelism", unix))]
benchmark_group!(benches, extract_basic, extract_split);

benchmark_main!(benches);
