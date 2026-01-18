#![cfg(feature = "aes-crypto")]

use std::io::{self, Read, Write};
use zip::write::ZipWriter;
#[cfg(feature = "deflate-flate2")]
use zip::CompressionMethod::Deflated;
use zip::{result::ZipError, write::SimpleFileOptions, AesMode, ZipArchive};

const SECRET_CONTENT: &str = "Lorem ipsum dolor sit amet";

const PASSWORD: &[u8] = b"helloworld";

#[test]
fn aes256_encrypted_uncompressed_file() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/aes_archive.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    let mut file = archive
        .by_name_decrypt("secret_data_256_uncompressed", PASSWORD)
        .expect("couldn't find file in archive");
    assert_eq!("secret_data_256_uncompressed", file.name());

    let mut decrypted_content = String::new();
    file.read_to_string(&mut decrypted_content)
        .expect("couldn't read encrypted file");
    assert_eq!(SECRET_CONTENT, decrypted_content);
}

#[test]
fn aes256_encrypted_file() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/aes_archive.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    let mut file = archive
        .by_name_decrypt("secret_data_256", PASSWORD)
        .expect("couldn't find file in archive");
    assert_eq!("secret_data_256", file.name());

    let mut content = String::new();
    file.read_to_string(&mut content)
        .expect("couldn't read encrypted and compressed file");
    assert_eq!(SECRET_CONTENT, content);
}

#[test]
fn aes192_encrypted_file() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/aes_archive.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    let mut file = archive
        .by_name_decrypt("secret_data_192", PASSWORD)
        .expect("couldn't find file in archive");
    assert_eq!("secret_data_192", file.name());

    let mut content = String::new();
    file.read_to_string(&mut content)
        .expect("couldn't read encrypted file");
    assert_eq!(SECRET_CONTENT, content);
}

#[test]
fn aes128_encrypted_file() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/aes_archive.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    let mut file = archive
        .by_name_decrypt("secret_data_128", PASSWORD)
        .expect("couldn't find file in archive");
    assert_eq!("secret_data_128", file.name());

    let mut content = String::new();
    file.read_to_string(&mut content)
        .expect("couldn't read encrypted file");
    assert_eq!(SECRET_CONTENT, content);
}

#[test]
fn aes128_stored_roundtrip() {
    let cursor = {
        let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));

        zip.start_file(
            "test.txt",
            SimpleFileOptions::default().with_aes_encryption(AesMode::Aes128, "some password"),
        )
        .unwrap();
        zip.write_all(SECRET_CONTENT.as_bytes()).unwrap();

        zip.finish().unwrap()
    };

    let mut archive = ZipArchive::new(cursor).expect("couldn't open test zip file");
    test_extract_encrypted_file(&mut archive, "test.txt", "some password", "other password");
}

#[test]
#[cfg(feature = "deflate-flate2")]
fn aes256_deflated_roundtrip() {
    let cursor = {
        let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));

        zip.start_file(
            "test.txt",
            SimpleFileOptions::default()
                .compression_method(Deflated)
                .with_aes_encryption(AesMode::Aes256, "some password"),
        )
        .unwrap();
        zip.write_all(SECRET_CONTENT.as_bytes()).unwrap();

        zip.finish().unwrap()
    };

    let mut archive = ZipArchive::new(cursor).expect("couldn't open test zip file");
    test_extract_encrypted_file(&mut archive, "test.txt", "some password", "other password");
}

fn test_extract_encrypted_file<R: io::Read + io::Seek>(
    archive: &mut ZipArchive<R>,
    file_name: &str,
    correct_password: &str,
    incorrect_password: &str,
) {
    {
        let file = archive.by_name(file_name).map(|_| ());
        match file {
            Err(ZipError::UnsupportedArchive("Password required to decrypt file")) => {}
            Err(err) => {
                panic!("Failed to read file for unknown reason: {err:?}");
            }
            Ok(_) => {
                panic!("Was able to successfully read encrypted file without password");
            }
        }
    }

    {
        match archive.by_name_decrypt(file_name, incorrect_password.as_bytes()) {
            Err(ZipError::InvalidPassword) => {}
            Err(err) => panic!("Expected invalid password error, got: {err:?}"),
            Ok(_) => panic!("Expected invalid password, got decrypted file"),
        }
    }

    {
        let mut content = String::new();
        archive
            .by_name_decrypt(file_name, correct_password.as_bytes())
            .expect("couldn't read encrypted file")
            .read_to_string(&mut content)
            .expect("couldn't read encrypted file");
        assert_eq!(SECRET_CONTENT, content);
    }
}

#[test]
fn raw_copy_from_aes_zip() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/aes_archive.zip"));

    let dst_cursor = {
        let mut src =
            ZipArchive::new(io::Cursor::new(v.as_slice())).expect("couldn't open source zip");
        let mut dst = ZipWriter::new(io::Cursor::new(Vec::new()));

        let total = src.len();
        for i in 0..total {
            let file = src.by_index_raw(i).expect("read source entry");
            let name = file.name().to_string();
            if file.is_dir() {
                dst.add_directory(&name, SimpleFileOptions::default())
                    .expect("add directory");
            } else {
                dst.raw_copy_file(file).expect("raw copy file");
            }
        }
        dst.finish().expect("finish dst")
    };

    let mut src_zip = ZipArchive::new(io::Cursor::new(v.as_slice())).expect("reopen src zip");
    let mut dst_zip = ZipArchive::new(dst_cursor).expect("reopen dst zip");

    let total = src_zip.len();

    for i in 0..total {
        // Copy out simple header fields without holding borrows across later reads
        let (name, is_dir, s_encrypted, d_encrypted, s_comp, d_comp) = {
            let s = src_zip.by_index_raw(i).expect("src by_index_raw");
            let name = s.name().to_string();
            let is_dir = s.is_dir();
            let s_encrypted = s.encrypted();
            let s_comp = s.compression();
            let d = dst_zip.by_index_raw(i).expect("dst by_index_raw");
            let d_encrypted = d.encrypted();
            let d_comp = d.compression();
            (name, is_dir, s_encrypted, d_encrypted, s_comp, d_comp)
        };

        // AES-critical invariants preserved by raw copy
        assert_eq!(
            s_encrypted, d_encrypted,
            "encrypted flag differs for {name}"
        );
        assert_eq!(s_comp, d_comp, "compression method differs for {name}");

        // For files, verify content bytes match. For encrypted entries, use the shared fixture password
        if !is_dir {
            let mut s_buf = Vec::new();
            let mut d_buf = Vec::new();
            if s_encrypted {
                src_zip
                    .by_index_decrypt(i, PASSWORD)
                    .expect("decrypt src")
                    .read_to_end(&mut s_buf)
                    .expect("read src");
                dst_zip
                    .by_index_decrypt(i, PASSWORD)
                    .expect("decrypt dst")
                    .read_to_end(&mut d_buf)
                    .expect("read dst");
            } else {
                src_zip
                    .by_index(i)
                    .expect("open src")
                    .read_to_end(&mut s_buf)
                    .expect("read src");
                dst_zip
                    .by_index(i)
                    .expect("open dst")
                    .read_to_end(&mut d_buf)
                    .expect("read dst");
            }
            assert_eq!(s_buf, d_buf, "content differs for {name}");
        }
    }
}
