#[cfg(all(unix, feature = "deflate-flate2"))]
#[test]
fn repro_old423() -> zip::result::ZipResult<()> {
    use std::io;
    use tempfile::TempDir;
    use zip::ZipArchive;

    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/lin-ub_iwd-v11.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");
    archive.extract(TempDir::with_prefix("repro_old423")?)
}

#[cfg(all(unix, feature = "parallelism", feature = "_deflate-any"))]
#[test]
fn repro_old423_pipelined() -> zip::result::ZipResult<()> {
    use std::{fs, path::Path};
    use tempdir::TempDir;
    use zip::{read::split_extract, ZipArchive};

    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/lin-ub_iwd-v11.zip");
    let file = fs::File::open(path)?;
    let archive = ZipArchive::new(file)?;
    let td = TempDir::new("repro_old423")?;
    split_extract(&archive, td.path(), Default::default()).expect("couldn't extract test zip");
    Ok(())
}
