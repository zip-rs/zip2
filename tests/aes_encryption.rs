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

const GENUINE: &[u8] = b"balance: 100 USD\n";
const FORGED: &[u8] = b"balance: 999 USD\n";
const AES256_OVERHEAD: usize = 28; // 16-byte salt + 2-byte verifier + 10-byte tag

fn genuine_archive(plaintext: &[u8]) -> Vec<u8> {
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    use zip::{AesMode, CompressionMethod, ZipWriter};
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .with_aes_encryption_bytes(AesMode::Aes256, PASSWORD);
    w.start_file("secret.txt", opts).unwrap();
    w.write_all(plaintext).unwrap();
    w.finish().unwrap().into_inner()
}

/// What a victim does: open with the correct password and read the entry.
fn victim_reads(archive: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::io::{Cursor, Read};
    use zip::ZipArchive;
    let mut a = ZipArchive::new(Cursor::new(archive.to_vec())).unwrap();
    let mut f = a.by_index_decrypt(0, PASSWORD).unwrap();
    let mut out = Vec::new();
    f.read_to_end(&mut out)?;
    Ok(out)
}

fn u16le(b: &[u8], off: usize) -> usize {
    u16::from_le_bytes([b[off], b[off + 1]]) as usize
}

/// Payload offset of the single local file header at offset 0.
fn payload_start(a: &[u8]) -> usize {
    30 + u16le(a, 26) + u16le(a, 28)
}

/// Control: on a well-formed archive the HMAC does reject forged ciphertext.
#[test]
fn tampering_is_detected_when_the_declared_size_is_honest() {
    let mut a = genuine_archive(GENUINE);
    let ct = payload_start(&a) + 18;
    for i in 0..GENUINE.len() {
        a[ct + i] ^= GENUINE[i] ^ FORGED[i]; // AES-CTR is malleable
    }
    let err = victim_reads(&a).unwrap_err();
    assert!(err.to_string().contains("Invalid authentication code"));
}

/// The bypass: chosen plaintext is delivered verbatim, without the password.
#[test]
fn hmac_is_bypassed_when_the_entry_ends_early() {
    let genuine = genuine_archive(GENUINE);
    let hdr_len = payload_start(&genuine);
    let eocd = genuine.len() - 22; // no archive comment yet
    let cd_offset = u32::from_le_bytes(genuine[eocd + 16..eocd + 20].try_into().unwrap()) as usize;

    // Park a second copy of the entry after the end-of-central-directory record,
    // inside the archive comment, so its data ends at true EOF. Header, salt and
    // password verifier are copied verbatim — no password knowledge is required.
    let mut evil = genuine.clone();
    let new_hdr_offset = evil.len() as u32;
    evil.extend_from_slice(&genuine[..hdr_len + 18]); // local header + salt + verifier
    for i in 0..GENUINE.len() {
        // XOR the keystream onto the chosen plaintext; the 10-byte tag is omitted.
        evil.push(genuine[hdr_len + 18 + i] ^ GENUINE[i] ^ FORGED[i]);
    }
    let comment_len = (evil.len() - genuine.len()) as u16;
    evil[eocd + 20..eocd + 22].copy_from_slice(&comment_len.to_le_bytes());

    // Repoint the central directory at the parked copy and declare more data than
    // is present, so `data_remaining` can never reach zero.
    let declared = (AES256_OVERHEAD + GENUINE.len() + 64) as u32;
    evil[cd_offset + 42..cd_offset + 46].copy_from_slice(&new_hdr_offset.to_le_bytes());
    evil[cd_offset + 20..cd_offset + 24].copy_from_slice(&declared.to_le_bytes());
    let hdr2 = new_hdr_offset as usize;
    evil[hdr2 + 18..hdr2 + 22].copy_from_slice(&declared.to_le_bytes());

    let read_result = victim_reads(&evil);
    // unauthenticated content was detected
    assert!(read_result.is_err());
}

/// Second path: an entry with no payload bytes is never authenticated at all.
#[test]
fn hmac_is_never_checked_for_a_zero_length_entry() {
    let mut a = genuine_archive(b"");
    let tag = payload_start(&a) + 18;
    a[tag] ^= 0xff; // corrupt the 10-byte authentication code
    let read_result = victim_reads(&a);
    // corrupt authentication code was detected
    assert!(read_result.is_err());
}

/// Reading an empty AES entry again after it returned Ok(0) must keep returning Ok(0), like a
/// non-empty entry. The HMAC of an empty entry was verified on every read, so the second read
/// failed with "Tried to use an already finalized HMAC" (and hit a debug_assert in debug builds).
#[test]
fn aes_empty_entry_reread_at_eof() {
    for data in [&b""[..], SECRET_CONTENT.as_bytes()] {
        let mut w = ZipWriter::new(io::Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .with_aes_encryption_bytes(AesMode::Aes256, PASSWORD);
        w.start_file("f", opts).unwrap();
        w.write_all(data).unwrap();
        let mut archive = ZipArchive::new(w.finish().unwrap()).unwrap();
        let mut file = archive.by_index_decrypt(0, PASSWORD).unwrap();

        let mut out = Vec::new();
        file.read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
        let mut buf = [0u8; 16];
        assert_eq!(file.read(&mut buf).unwrap(), 0);
        assert_eq!(file.read(&mut buf).unwrap(), 0);
    }
}

/// GHSA-c9rm-qm35-rhqf: the HMAC must be verified even when the decompressor
/// reports end-of-stream before all the AES ciphertext has been consumed.
mod early_eof {
    use super::PASSWORD;
    use std::io::{self, Cursor, Read, Write};
    use zip::write::SimpleFileOptions;
    use zip::{AesMode, CompressionMethod, ZipArchive, ZipWriter};

    const KNOWN: &[u8] = b"KNOWN-PLAINTEXT\n";
    const FORGED: &[u8] = b"FORGED: attacker-chosen content, written without the password\n";

    fn known_plaintext(len: usize) -> Vec<u8> {
        (0..len).map(|i| KNOWN[i % KNOWN.len()]).collect()
    }

    fn pseudo_random(len: usize) -> Vec<u8> {
        let mut x: u32 = 0x1234_5678;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    fn u16le(b: &[u8], off: usize) -> usize {
        u16::from_le_bytes([b[off], b[off + 1]]) as usize
    }

    fn u32le(b: &[u8], off: usize) -> usize {
        u32::from_le_bytes(b[off..off + 4].try_into().unwrap()) as usize
    }

    fn put_u32(b: &mut [u8], off: usize, v: usize) {
        b[off..off + 4].copy_from_slice(&(v as u32).to_le_bytes());
    }

    /// Single-entry archive written by the crate itself.
    fn aes_archive(plaintext: &[u8], method: CompressionMethod) -> Vec<u8> {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default()
            .compression_method(method)
            .with_aes_encryption_bytes(AesMode::Aes256, PASSWORD);
        w.start_file("doc.txt", opts).unwrap();
        w.write_all(plaintext).unwrap();
        w.finish().unwrap().into_inner()
    }

    /// Offset of the (single) central directory header, taken from the EOCD.
    fn cd_offset(b: &[u8]) -> usize {
        u32le(b, b.len() - 22 + 16)
    }

    fn payload_start(b: &[u8]) -> usize {
        30 + u16le(b, 26) + u16le(b, 28)
    }

    /// Visit the AES extra field (0x9901) in both the local and central headers.
    fn for_each_aes_extra(b: &mut [u8], mut f: impl FnMut(&mut [u8])) {
        let cd = cd_offset(b);
        let regions = [
            (30 + u16le(b, 26), u16le(b, 28)),
            (cd + 46 + u16le(b, cd + 28), u16le(b, cd + 30)),
        ];
        for (start, len) in regions {
            let mut p = start;
            while p + 4 <= start + len {
                let (id, size) = (u16le(b, p), u16le(b, p + 2));
                if id == 0x9901 {
                    f(&mut b[p + 4..p + 4 + size]);
                }
                p += 4 + size;
            }
        }
    }

    /// Rewrite AES vendor version (1 = AE-1, 2 = AE-2) and/or the real
    /// compression method stored in the AES extra field. For AE-1 the CRC of
    /// `crc_of` is written to both headers.
    fn set_aes(b: &mut [u8], vendor: Option<u16>, method: Option<u16>, crc_of: &[u8]) {
        for_each_aes_extra(b, |e| {
            if let Some(v) = vendor {
                e[0..2].copy_from_slice(&v.to_le_bytes());
            }
            if let Some(m) = method {
                e[5..7].copy_from_slice(&m.to_le_bytes());
            }
        });
        if vendor == Some(1) {
            let crc = crc32fast::hash(crc_of) as usize;
            let cd = cd_offset(b);
            put_u32(b, 14, crc);
            put_u32(b, cd + 16, crc);
        }
    }

    fn set_uncompressed_size(b: &mut [u8], size: usize) {
        let cd = cd_offset(b);
        put_u32(b, 22, size);
        put_u32(b, cd + 24, size);
    }

    /// The attack from the advisory: no password is used. Starting from a
    /// genuine Stored AES entry with known plaintext, XOR a tiny raw-deflate
    /// stream over the start of the ciphertext and relabel the entry as
    /// Deflate. The deflate decoder stops after the forged block, leaving most
    /// of the (unmodified) ciphertext unread.
    fn forged_archive(entry_len: usize, vendor: u16) -> Vec<u8> {
        let p = known_plaintext(entry_len);
        let mut b = aes_archive(&p, CompressionMethod::Stored);
        let mut d = vec![0x01, FORGED.len() as u8, 0, !(FORGED.len() as u8), 0xff];
        d.extend_from_slice(FORGED);
        let ct = payload_start(&b) + 16 + 2; // salt + password verifier
        for i in 0..d.len() {
            b[ct + i] ^= p[i] ^ d[i];
        }
        set_aes(&mut b, Some(vendor), Some(8), FORGED);
        set_uncompressed_size(&mut b, FORGED.len());
        b
    }

    /// Genuine Deflate AES entry with `n` extra bytes inserted between the
    /// end of the deflate stream and the authentication code (sizes and
    /// offsets adjusted, HMAC not recomputed).
    fn trailing_garbage_archive(n: usize) -> Vec<u8> {
        let p = known_plaintext(4096);
        let b = aes_archive(&p, CompressionMethod::Deflated);
        let ps = payload_start(&b);
        let cs = u32le(&b, 18);
        let tag = ps + cs - 10;
        let cd = cd_offset(&b);
        let mut out = Vec::with_capacity(b.len() + n);
        out.extend_from_slice(&b[..tag]);
        out.extend(pseudo_random(n));
        out.extend_from_slice(&b[tag..]);
        put_u32(&mut out, 18, cs + n);
        let cd = cd + n;
        put_u32(&mut out, cd + 20, cs + n);
        let eocd = out.len() - 22;
        put_u32(&mut out, eocd + 16, cd);
        out
    }

    fn assert_auth_error(e: &io::Error) {
        assert_eq!(e.kind(), io::ErrorKind::InvalidData, "{e}");
        assert!(e.to_string().contains("Invalid authentication code"), "{e}");
    }

    #[derive(Clone, Copy, Debug)]
    enum How {
        ReadToEnd,
        ReadToString,
        ReadLoop,
    }

    fn read_all(r: &mut impl Read, how: How) -> io::Result<Vec<u8>> {
        match how {
            How::ReadToEnd => {
                let mut v = Vec::new();
                r.read_to_end(&mut v)?;
                Ok(v)
            }
            How::ReadToString => {
                let mut s = String::new();
                r.read_to_string(&mut s)?;
                Ok(s.into_bytes())
            }
            How::ReadLoop => {
                // small buffer to exercise many `read` calls (like io::copy)
                let mut v = Vec::new();
                let mut buf = [0u8; 100];
                loop {
                    let n = r.read(&mut buf)?;
                    if n == 0 {
                        return Ok(v);
                    }
                    v.extend_from_slice(&buf[..n]);
                }
            }
        }
    }

    const ALL: [How; 3] = [How::ReadToEnd, How::ReadToString, How::ReadLoop];

    #[test]
    fn forged_deflate_stream_is_rejected() {
        for vendor in [1, 2] {
            for len in [12 * 1024, 64 * 1024] {
                for how in ALL {
                    let b = forged_archive(len, vendor);
                    let mut a = ZipArchive::new(Cursor::new(b)).unwrap();
                    let mut f = a.by_index_decrypt(0, PASSWORD).unwrap();
                    let r = read_all(&mut f, how).map(|v| v.len());
                    let e = r.expect_err(&format!(
                        "AE-{vendor} len={len} {how:?}: forged entry accepted"
                    ));
                    assert_auth_error(&e);
                    // The failure must be sticky: a retry must not report a clean EOF.
                    assert!(
                        f.read(&mut [0u8; 16]).is_err(),
                        "AE-{vendor} {how:?}: error not sticky"
                    );
                }
            }
        }
    }

    #[test]
    fn forged_deflate_stream_is_rejected_when_streaming() {
        for vendor in [1, 2] {
            let b = forged_archive(64 * 1024, vendor);
            let mut cur = Cursor::new(b);
            let opts = zip::ZipReadOptions::new().password(Some(PASSWORD));
            let mut f = zip::read::read_zipfile_from_stream_with_options(&mut cur, opts)
                .unwrap()
                .unwrap();
            let e = read_all(&mut f, How::ReadToEnd)
                .map(|v| v.len())
                .expect_err("forged entry accepted");
            assert_auth_error(&e);
        }
    }

    #[test]
    fn trailing_data_inside_compressed_size_is_authenticated() {
        for n in [16, 20 * 1024] {
            for how in ALL {
                let b = trailing_garbage_archive(n);
                let mut a = ZipArchive::new(Cursor::new(b)).unwrap();
                let mut f = a.by_index_decrypt(0, PASSWORD).unwrap();
                let e = read_all(&mut f, how).map(|v| v.len()).expect_err(&format!(
                    "n={n} {how:?}: unauthenticated trailing data accepted"
                ));
                assert_auth_error(&e);
            }
        }
    }

    #[test]
    fn valid_aes_entries_still_read() {
        let contents: [Vec<u8>; 5] = [
            Vec::new(),
            b"asdf\n".to_vec(),
            known_plaintext(64 * 1024),
            pseudo_random(64 * 1024),
            pseudo_random(5000),
        ];
        #[allow(unused_mut)]
        let mut methods = vec![CompressionMethod::Stored, CompressionMethod::Deflated];
        #[cfg(feature = "bzip2")]
        methods.push(CompressionMethod::Bzip2);
        #[cfg(feature = "zstd")]
        methods.push(CompressionMethod::Zstd);
        #[cfg(feature = "xz")]
        methods.push(CompressionMethod::Xz);
        #[cfg(feature = "ppmd")]
        methods.push(CompressionMethod::Ppmd);
        for method in methods {
            for vendor in [1u16, 2] {
                for p in &contents {
                    for how in [How::ReadToEnd, How::ReadLoop] {
                        let mut b = aes_archive(p, method);
                        set_aes(&mut b, Some(vendor), None, p);
                        let mut a = ZipArchive::new(Cursor::new(b)).unwrap();
                        let mut f = a.by_index_decrypt(0, PASSWORD).unwrap();
                        let out = read_all(&mut f, how).unwrap_or_else(|e| {
                            panic!("{method:?} AE-{vendor} len={} {how:?}: {e}", p.len())
                        });
                        assert_eq!(&out, p);
                        // reading again after EOF keeps returning Ok(0)
                        assert_eq!(f.read(&mut [0u8; 16]).unwrap(), 0);
                    }
                }
            }
        }
    }

    #[test]
    fn valid_aes_entries_still_read_when_streaming() {
        for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
            let p1 = pseudo_random(40 * 1024);
            let p2 = known_plaintext(30 * 1024);
            let mut w = ZipWriter::new(Cursor::new(Vec::new()));
            let opts = SimpleFileOptions::default()
                .compression_method(method)
                .with_aes_encryption_bytes(AesMode::Aes256, PASSWORD);
            w.start_file("a", opts).unwrap();
            w.write_all(&p1).unwrap();
            w.start_file("b", opts).unwrap();
            w.write_all(&p2).unwrap();
            let mut cur = Cursor::new(w.finish().unwrap().into_inner());
            for expected in [&p1, &p2] {
                let opts = zip::ZipReadOptions::new().password(Some(PASSWORD));
                let mut f = zip::read::read_zipfile_from_stream_with_options(&mut cur, opts)
                    .unwrap()
                    .unwrap();
                assert_eq!(&read_all(&mut f, How::ReadToEnd).unwrap(), expected);
            }
        }
    }

    /// Callers that stop early are not penalised: no HMAC check, no error,
    /// and the next entry in a stream is still reachable.
    #[test]
    fn early_drop_is_not_an_error() {
        let p = pseudo_random(64 * 1024);
        for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
            let b = aes_archive(&p, method);
            let mut a = ZipArchive::new(Cursor::new(b)).unwrap();
            let mut f = a.by_index_decrypt(0, PASSWORD).unwrap();
            let mut buf = [0u8; 1000];
            f.read_exact(&mut buf).unwrap();
            assert_eq!(&buf[..], &p[..1000]);
            drop(f);
            // a fresh handle reads the whole entry fine
            let mut f = a.by_index_decrypt(0, PASSWORD).unwrap();
            assert_eq!(read_all(&mut f, How::ReadToEnd).unwrap(), p);
        }
    }
}
