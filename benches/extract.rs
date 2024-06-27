use bencher::{benchmark_group, benchmark_main};

use bencher::Bencher;
use tempdir::TempDir;
use tempfile::tempfile;

use std::fs;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use zip::result::ZipResult;
use zip::write::ZipWriter;
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
fn static_test_archive() -> ZipResult<ZipArchive<fs::File>> {
    assert!(
        cfg!(feature = "bzip2"),
        "this test archive requires bzip2 support"
    );
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/stored-and-compressed-text.zip");
    let file = fs::File::open(path)?;
    ZipArchive::new(file)
}

static STATIC_TEST_ARCHIVE: LazyLock<Mutex<ZipArchive<fs::File>>> = LazyLock::new(|| {
    let archive = static_test_archive().unwrap();
    Mutex::new(archive)
});

/* This archive is generated dynamically, in order to scale with the number of reported CPUs.
 * - We want at least 768 files (4 per VCPU on EC2 *.48xlarge instances) to run in CI.
 * - We want to retain the interspersed random/text entries from static_test_archive().
 *
 * We will copy over entries from the static archive repeatedly until we reach the desired file
 * count.
 */
fn dynamic_test_archive(src_archive: &mut ZipArchive<fs::File>) -> ZipResult<ZipArchive<fs::File>> {
    let desired_num_entries: usize = num_cpus::get() * 4;
    let mut output_archive = ZipWriter::new(tempfile()?);

    for (src_index, output_index) in (0..src_archive.len()).cycle().zip(0..desired_num_entries) {
        let src_file = src_archive.by_index_raw(src_index)?;
        let output_name = if src_file.name().starts_with("random-") {
            format!("random-{output_index}.dat")
        } else {
            assert!(src_file.name().starts_with("text-"));
            format!("text-{output_index}.dat")
        };
        output_archive.raw_copy_file_rename(src_file, output_name)?;
    }

    output_archive.finish_into_readable()
}

static DYNAMIC_TEST_ARCHIVE: LazyLock<Mutex<ZipArchive<fs::File>>> = LazyLock::new(|| {
    let mut src = STATIC_TEST_ARCHIVE.lock().unwrap();
    let archive = dynamic_test_archive(&mut src).unwrap();
    Mutex::new(archive)
});

fn do_extract_basic(bench: &mut Bencher, archive: &mut ZipArchive<fs::File>) {
    let total_size: u64 = archive.decompressed_size().unwrap().try_into().unwrap();

    let parent = TempDir::new("zip-extract").unwrap();

    bench.bytes = total_size;
    bench.bench_n(1, |bench| {
        bench.iter(move || {
            let outdir = TempDir::new_in(parent.path(), "bench-subdir")
                .unwrap()
                .into_path();
            archive.extract(outdir).unwrap();
        });
    });
}

fn extract_basic_static(bench: &mut Bencher) {
    let mut archive = STATIC_TEST_ARCHIVE.lock().unwrap();
    do_extract_basic(bench, &mut archive);
}

fn extract_basic_dynamic(bench: &mut Bencher) {
    let mut archive = DYNAMIC_TEST_ARCHIVE.lock().unwrap();
    do_extract_basic(bench, &mut archive);
}

#[cfg(all(feature = "parallelism", unix))]
fn do_extract_split(bench: &mut Bencher, archive: &ZipArchive<fs::File>) {
    let total_size: u64 = archive.decompressed_size().unwrap().try_into().unwrap();

    let params = ExtractionParameters {
        decompression_threads: num_cpus::get() / 3,
        ..Default::default()
    };

    let parent = TempDir::new("zip-extract").unwrap();

    bench.bytes = total_size;
    bench.bench_n(1, |bench| {
        bench.iter(move || {
            let outdir = TempDir::new_in(parent.path(), "bench-subdir")
                .unwrap()
                .into_path();
            split_extract(archive, &outdir, params.clone()).unwrap();
        });
    });
}

#[cfg(all(feature = "parallelism", unix))]
fn extract_split_static(bench: &mut Bencher) {
    let archive = STATIC_TEST_ARCHIVE.lock().unwrap();
    do_extract_split(bench, &archive);
}

#[cfg(all(feature = "parallelism", unix))]
fn extract_split_dynamic(bench: &mut Bencher) {
    let archive = DYNAMIC_TEST_ARCHIVE.lock().unwrap();
    do_extract_split(bench, &archive);
}

#[cfg(not(all(feature = "parallelism", unix)))]
benchmark_group!(benches, extract_basic_static, extract_basic_dynamic);

#[cfg(all(feature = "parallelism", unix))]
benchmark_group!(
    benches,
    extract_basic_static,
    extract_basic_dynamic,
    extract_split_static,
    extract_split_dynamic
);

benchmark_main!(benches);
