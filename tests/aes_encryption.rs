#![cfg(feature = "aes-crypto")]

use std::io::{self, Read, Write};
use zip::CompressionMethod;
use zip::write::ZipWriter;
use zip::{AesMode, ZipArchive, result::ZipError, write::SimpleFileOptions};

const SECRET_CONTENT: &str = "Lorem ipsum dolor sit amet";

const PASSWORD: &[u8] = b"helloworld";
const SOME_PASSWORD: &[u8] = b"some password";

#[test]
pub fn aes256_encrypted_uncompressed_file() {
    let mut archive = ZipArchive::new(io::Cursor::new(include_bytes!("data/aes_archive.zip")))
        .expect("couldn't open test zip file");

    let mut file = archive
        .by_name_decrypt("secret_data_256_uncompressed", PASSWORD)
        .expect("couldn't find file in archive");
    let file_name = file.name().unwrap();
    assert_eq!("secret_data_256_uncompressed", file_name);

    let mut decrypted_content = String::new();
    file.read_to_string(&mut decrypted_content)
        .expect("couldn't read encrypted file");
    assert_eq!(SECRET_CONTENT, decrypted_content);
}

#[test]
fn aes256_encrypted_file() {
    let mut archive = ZipArchive::new(io::Cursor::new(include_bytes!("data/aes_archive.zip")))
        .expect("couldn't open test zip file");

    let mut file = archive
        .by_name_decrypt("secret_data_256", PASSWORD)
        .expect("couldn't find file in archive");
    let file_name = file.name().unwrap();
    assert_eq!("secret_data_256", file_name);

    let mut content = String::new();
    file.read_to_string(&mut content)
        .expect("couldn't read encrypted and compressed file");
    assert_eq!(SECRET_CONTENT, content);
}

#[test]
fn aes192_encrypted_file() {
    let mut archive = ZipArchive::new(io::Cursor::new(include_bytes!("data/aes_archive.zip")))
        .expect("couldn't open test zip file");

    let mut file = archive
        .by_name_decrypt("secret_data_192", PASSWORD)
        .expect("couldn't find file in archive");
    let file_name = file.name().unwrap();
    assert_eq!("secret_data_192", file_name);

    let mut content = String::new();
    file.read_to_string(&mut content)
        .expect("couldn't read encrypted file");
    assert_eq!(SECRET_CONTENT, content);
}

#[test]
fn aes128_encrypted_file() {
    let mut archive = ZipArchive::new(io::Cursor::new(include_bytes!("data/aes_archive.zip")))
        .expect("couldn't open test zip file");

    let mut file = archive
        .by_name_decrypt("secret_data_128", PASSWORD)
        .expect("couldn't find file in archive");
    let file_name = file.name().unwrap();
    assert_eq!("secret_data_128", file_name);

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
    use zip::CompressionMethod::Deflated;
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
        let mut file = archive
            .by_name_decrypt(file_name, correct_password.as_bytes())
            .expect("couldn't retrieve encrypted file");
        file.read_to_string(&mut content)
            .expect("couldn't read to string encrypted file");
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
            let name = file.name().unwrap().to_string();
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
            let name = s.name().unwrap().to_string();
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

#[test]
fn aes_custom_salt_for_reproducible_zip() {
    use zip::AesSalt;
    use zip::DateTime;

    for (mode, salt, expected_error) in [
        (AesMode::Aes128, [1, 2, 3, 4, 5, 6, 7, 8].to_vec(), None),
        (
            AesMode::Aes128,
            [].into(), // salt too short
            Some("Salt for AES-128 must be 8 bytes long: could not convert slice to array"),
        ),
        (
            AesMode::Aes128,
            [1, 2, 3, 4, 5, 6, 7, 8, 9].into(), // salt too long should be rejected
            Some("Salt for AES-128 must be 8 bytes long: could not convert slice to array"),
        ),
        (
            AesMode::Aes192,
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12].into(),
            None,
        ),
        (
            AesMode::Aes192,
            [].into(), // salt too short
            Some("Salt for AES-192 must be 12 bytes long: could not convert slice to array"),
        ),
        (
            AesMode::Aes192,
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13].into(), // salt too long should be rejected
            Some("Salt for AES-192 must be 12 bytes long: could not convert slice to array"),
        ),
        (
            AesMode::Aes256,
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16].into(),
            None,
        ),
        (
            AesMode::Aes256,
            [1, 2, 3, 4, 5, 6, 7, 8].into(),
            Some("Salt for AES-256 must be 16 bytes long: could not convert slice to array"),
        ),
        (
            AesMode::Aes256,
            // salt too long should be rejected
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17].into(),
            Some("Salt for AES-256 must be 16 bytes long: could not convert slice to array"),
        ),
    ] {
        let custom_salt = AesSalt::try_new(mode, salt.as_slice());
        if let Some(expected_error) = expected_error {
            assert_eq!(custom_salt.unwrap_err().to_string(), expected_error);
            continue;
        }
        let custom_salt = custom_salt.expect("Failed to create custom salt");
        let options = SimpleFileOptions::default()
            .last_modified_time(DateTime::default())
            .with_aes_encryption_and_salt(SOME_PASSWORD, custom_salt);

        let mut data1 = Vec::new();
        let mut zip1 = ZipWriter::new(io::Cursor::new(&mut data1));
        zip1.start_file("test.txt", options).unwrap();
        let fake_file = [0u8; 16];
        let mut f = io::Cursor::new(fake_file);
        std::io::copy(&mut f, &mut zip1).unwrap();
        zip1.finish().unwrap();

        let mut data2 = Vec::new();
        let mut zip2 = ZipWriter::new(io::Cursor::new(&mut data2));
        zip2.start_file("test.txt", options).unwrap();
        let fake_file = [0u8; 16];
        let mut f = io::Cursor::new(fake_file);
        std::io::copy(&mut f, &mut zip2).unwrap();
        zip2.finish().unwrap();

        assert_eq!(
            data1, data2,
            "Expected identical zip contents for same salt"
        );
    }
}

#[test]
fn test_update_aes_version_on_threshold() {
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .with_aes_encryption(AesMode::Aes256, "some password");

    let mut data = Vec::new();
    let mut zip = ZipWriter::new(io::Cursor::new(&mut data));
    zip.start_file("test.txt", options).unwrap();
    zip.write_all(b"HELLO").unwrap();
    zip.finish().unwrap();

    // checksum is empty because we use version 2
    assert_eq!(data[14..18], [0, 0, 0, 0]); // checksum
    assert_eq!(data[18..22], [0x21, 0, 0, 0]); // compressed size
    assert_eq!(data[22..26], [5, 0, 0, 0]); // uncompressed size

    assert_eq!(&data[38..40], [1, 0x99]); // Header ID
    assert_eq!(&data[40..42], [7, 0]); // Size
    assert_eq!(&data[42..44], [2, 0]); // Version
    assert_eq!(&data[44..46], b"AE"); // Vendor ID
    assert_eq!(data[46], 3); // AES Mode
    assert_eq!(data[47..49], [0, 0]); // Compression Method

    // checksum is empty because we use version 2
    assert_eq!(data[98..102], [0, 0, 0, 0]); // checksum
    assert_eq!(data[102..106], [0x21, 0, 0, 0]); // compressed size
    assert_eq!(data[106..110], [5, 0, 0, 0]); // uncompressed size

    assert_eq!(&data[136..138], [1, 0x99]); // Header ID
    assert_eq!(&data[138..140], [7, 0]); // Size
    assert_eq!(&data[140..142], [2, 0]); // Version
    assert_eq!(&data[142..144], b"AE"); // Vendor ID
    assert_eq!(data[144], 3); // AES Mode
    assert_eq!(data[145..147], [0, 0]); // Compression Method

    // Redo the same but with a file more than 20 bytes
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .with_aes_encryption(AesMode::Aes256, "some password");
    let mut data = Vec::new();
    let mut zip = ZipWriter::new(io::Cursor::new(&mut data));
    zip.start_file("test.txt", options).unwrap();
    zip.write_all(b"LONG FILE MORE THAN 20 BYTES").unwrap();
    zip.finish().unwrap();

    assert_eq!(data[14..18], [0xb7, 0x81, 0xef, 0xad]); // checksum
    assert_eq!(data[18..22], [0x38, 0, 0, 0]); // compressed size
    assert_eq!(data[22..26], [0x1c, 0, 0, 0]); // uncompressed size

    assert_eq!(&data[38..40], [1, 0x99]); // Header ID
    assert_eq!(&data[40..42], [7, 0]); // Size
    assert_eq!(&data[42..44], [1, 0]); // Version
    assert_eq!(&data[44..46], b"AE"); // Vendor ID
    assert_eq!(data[46], 3); // AES Mode
    assert_eq!(data[47..49], [0, 0]); // Compression Method

    assert_eq!(data[121..125], [0xb7, 0x81, 0xef, 0xad]); // checksum
    assert_eq!(data[125..129], [0x38, 0, 0, 0]); // compressed size
    assert_eq!(data[129..133], [0x1c, 0, 0, 0]); // uncompressed size

    assert_eq!(&data[159..161], [1, 0x99]); // Header ID
    assert_eq!(&data[161..163], [7, 0]); // Size
    assert_eq!(&data[163..165], [1, 0]); // Version
    assert_eq!(&data[165..167], b"AE"); // Vendor ID
    assert_eq!(data[167], 3); // AES Mode
    assert_eq!(data[168..170], [0, 0]); // Compression Method
}
