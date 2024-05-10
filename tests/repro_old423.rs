use std::env::temp_dir;
use std::io;
use zip::result::ZipResult;
use zip::ZipArchive;

#[test]
fn repro_old423() -> ZipResult<()> {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/lin-ub_iwd-v11.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");
    archive.extract(temp_dir())
}