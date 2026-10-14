#[cfg(all(
    feature = "lzma",
    not(all(miri, any(target_arch = "x86_64", target_arch = "aarch64")))
))]
#[test]
pub fn decompress_lzma() {
    use std::io::{self, Read};
    use zip::ZipArchive;
    let v = include_bytes!("data/lzma.zip");
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    let mut file = archive
        .by_name("binary.wmv")
        .expect("couldn't find file in archive");
    let file_name = file.name().unwrap();
    assert_eq!("binary.wmv", file_name);

    #[cfg(not(miri))]
    {
        let mut content = Vec::new();
        file.read_to_end(&mut content)
            .expect("couldn't read encrypted and compressed file");
        assert_eq!(&include_bytes!("data/folder/binary.wmv")[..], &content[..]);
    }

    #[cfg(miri)]
    {
        let mut content = [0; 1024];
        file.read(&mut content)
            .expect("couldn't read encrypted and compressed file");
        assert_eq!(
            &include_bytes!("data/folder/binary.wmv")[0..1024],
            &content[..]
        );
    }
}
