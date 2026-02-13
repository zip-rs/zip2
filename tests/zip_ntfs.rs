use std::io;

use zip::ZipArchive;

#[test]
fn test_ntfs_extra_field_timestamp_parsing() {
    let mut archive = ZipArchive::new(io::Cursor::new(include_bytes!("../tests/data/ntfs.zip")))
        .expect("couldn't open test zip file");

    let file = archive
        .by_name("test.txt")
        .expect("expected file 'test.txt' to be present in ntfs.zip test archive");

    let timestamp = file
        .extra_data_fields()
        .find_map(|field| {
            if let zip::ExtraField::Ntfs(ts) = field {
                Some(ts)
            } else {
                None
            }
        })
        .expect("Expected NTFS extra field in test.txt");
}
