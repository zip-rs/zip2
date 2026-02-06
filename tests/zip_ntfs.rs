use std::io;

use zip::ZipArchive;

#[test]
fn test_ntfs_extra_field_timestamp_parsing() {
    let mut archive = ZipArchive::new(io::Cursor::new(include_bytes!("../tests/data/ntfs.zip")))
        .expect("couldn't open test zip file");

    let mut found_ntfs = false;

    for field in archive.by_name("test.txt").unwrap().extra_data_fields() {
        if let zip::ExtraField::Ntfs(ts) = field {
            found_ntfs = true;
            // Expected NTFS modification time of test.txt from ntfs.zip in Windows FILETIME units.
            assert_eq!(ts.mtime(), 133_813_273_144_169_390);
            #[cfg(feature = "nt-time")]
            assert_eq!(
                time::UtcDateTime::try_from(ts.modified_file_time()).unwrap(),
                time::macros::datetime!(2025-01-14 11:21:54.416_939_000 UTC)
            );

            assert_eq!(ts.atime(), 0);
            #[cfg(feature = "nt-time")]
            assert_eq!(ts.accessed_file_time(), nt_time::FileTime::NT_TIME_EPOCH);

            assert_eq!(ts.ctime(), 0);
            #[cfg(feature = "nt-time")]
            assert_eq!(ts.created_file_time(), nt_time::FileTime::NT_TIME_EPOCH);
        }
    }
    assert!(found_ntfs, "Expected NTFS extra field in test.txt");
}
