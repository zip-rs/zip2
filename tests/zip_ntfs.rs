use std::io;

use zip::ZipArchive;

#[test]
fn test_ntfs() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("../tests/data/ntfs.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    for field in archive.by_name("test.txt").unwrap().extra_data_fields() {
        if let zip::ExtraField::Ntfs(ts) = field {
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
}
