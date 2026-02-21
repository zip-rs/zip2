use std::io::{Cursor, Read, Write};
use std::path::Path;
use zip::read::ZipFile;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

const DIRECTORY_NAME: &str = "test_directory";
const FILE_NAME: &str = "hello_world.txt";
const LOREM_IPSUM: &[u8] = b"Lorem ipsum dolor sit amet, consectetur adipiscing elit.";

#[test]
fn by_path() {
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut archive = create_archive(options);
    let path = Path::new(DIRECTORY_NAME).join(FILE_NAME);
    let file = archive.by_path(path).unwrap();
    validate_file(file);
}

#[test]
#[cfg(feature = "aes-crypto")]
fn by_path_decrypt() {
    use zip::AesMode;

    const PASSWORD: &str = "helloworld";

    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .with_aes_encryption(AesMode::Aes128, PASSWORD);
    let mut archive = create_archive(options);
    let path = Path::new(DIRECTORY_NAME).join(FILE_NAME);
    let file = archive.by_path_decrypt(path, PASSWORD.as_bytes()).unwrap();
    validate_file(file);
}

#[test]
#[cfg(feature = "aes-crypto")]
fn by_path_decrypt_and_bytes_password() {
    use zip::AesMode;

    const PASSWORD: &str = "helloworld";

    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .with_aes_encryption_bytes(AesMode::Aes128, PASSWORD.as_bytes()); // change
    let mut archive = create_archive(options);
    let path = Path::new(DIRECTORY_NAME).join(FILE_NAME);
    let file = archive.by_path_decrypt(path, PASSWORD.as_bytes()).unwrap();
    validate_file(file);
}

fn create_archive(options: SimpleFileOptions) -> ZipArchive<Cursor<Vec<u8>>> {
    let mut buf = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(&mut buf));
    zip.add_directory(DIRECTORY_NAME, options).unwrap();
    zip.start_file(format!("{DIRECTORY_NAME}/{FILE_NAME}"), options)
        .unwrap();
    zip.write_all(LOREM_IPSUM).unwrap();
    zip.finish().unwrap();
    ZipArchive::new(Cursor::new(buf)).unwrap()
}

fn validate_file<T>(mut file: ZipFile<T>)
where
    T: Read,
{
    let mut file_buf = Vec::new();
    file.read_to_end(&mut file_buf).unwrap();
    assert_eq!(LOREM_IPSUM, file_buf);
}
