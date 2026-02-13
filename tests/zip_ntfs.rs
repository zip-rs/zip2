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

    assert_eq!(timestamp.mtime(), 133_813_273_144_169_390);
    #[cfg(feature = "nt-time")]
    assert_eq!(
        time::UtcDateTime::try_from(timestamp.modified_file_time()).unwrap(),
        time::macros::datetime!(2025-01-14 11:21:54.416_939_000 UTC)
    );

    assert_eq!(timestamp.atime(), 0);
    #[cfg(feature = "nt-time")]
    assert_eq!(timestamp.accessed_file_time(), nt_time::FileTime::NT_TIME_EPOCH);

    assert_eq!(timestamp.ctime(), 0);
    #[cfg(feature = "nt-time")]
    assert_eq!(timestamp.created_file_time(), nt_time::FileTime::NT_TIME_EPOCH);
}
