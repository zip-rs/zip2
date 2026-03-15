use std::io::Cursor;
use zip::ZipArchive;

#[test]
fn test_prepended_garbage() {
    let mut v = vec![0, 1, 2, 3];
    v.extend_from_slice(include_bytes!("../tests/data/extended_timestamp.zip"));

    let mut archive = ZipArchive::new(Cursor::new(v)).expect("couldn't open test zip file");

    assert_eq!(2, archive.len());

    for file_idx in 0..archive.len() {
        let file = archive.by_index(file_idx).unwrap();
        let outpath = file.enclosed_name().unwrap();

        println!(
            "Entry {} has name \"{}\" ({} bytes)",
            file_idx,
            outpath.display(),
            file.size()
        );
    }
}
