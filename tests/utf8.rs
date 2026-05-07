#[test]
#[cfg(feature = "deflate-flate2")]
fn test_utf8_extra_field() {
    use zip::ZipArchive;
    use std::io::Cursor;

    let mut reader =
        ZipArchive::new(Cursor::new(include_bytes!("../tests/data/chinese.zip"))).unwrap();
    reader.by_name("七个房间.txt").unwrap();
}

#[test]
fn test_utf8() {
    use zip::ZipArchive;
    use std::io::Cursor;

    let mut reader =
        ZipArchive::new(Cursor::new(include_bytes!("../tests/data/linux-7z.zip"))).unwrap();
    reader.by_name("你好.txt").unwrap();
}

#[test]
fn test_utf8_2() {
    use zip::ZipArchive;
    use std::io::Cursor;

    let mut reader = ZipArchive::new(Cursor::new(include_bytes!(
                "../tests/data/windows-7zip.zip"
    )))
        .unwrap();
    reader.by_name("你好.txt").unwrap();
}

