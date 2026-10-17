use std::io::{Cursor, Read, Seek, Write};
use std::path::Path;

use zip::result::ZipError;
use zip::unstable::write::FileOptionsExt;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const PAYLOAD: &[u8] = b"synthetic-content\n";
const PASSWORD_LENGTH: usize = 32;

fn fixture_password() -> [u8; PASSWORD_LENGTH] {
    #[cfg(not(target_arch = "wasm32"))]
    {
        rand::random::<[u8; PASSWORD_LENGTH]>()
    }
    #[cfg(target_arch = "wasm32")]
    {
        std::array::from_fn(|_| {
            getrandom::u32()
                .expect("Failed to generate fixture password")
                .to_le_bytes()[0]
        })
    }
}

fn archive(
    entries: &[(&str, &[u8])],
    password: Option<&[u8]>,
) -> TestResult<ZipArchive<Cursor<Vec<u8>>>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let mut options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    if let Some(password) = password {
        options = options.with_deprecated_encryption(password)?;
    }
    for (name, contents) in entries {
        writer.start_file(*name, options)?;
        writer.write_all(contents)?;
    }
    let mut bytes = writer.finish()?.into_inner();
    replace_cp437_name(&mut bytes, password.is_some());
    Ok(ZipArchive::new(Cursor::new(bytes))?)
}

fn replace_cp437_name(bytes: &mut [u8], encrypted: bool) {
    let marker = b"cafe.txt";
    let positions: Vec<_> = bytes
        .windows(marker.len())
        .enumerate()
        .filter_map(|(index, value)| (value == marker).then_some(index))
        .collect();
    assert_eq!(positions.len(), 2);
    for index in positions {
        let flags_offset = if &bytes[index - 30..index - 26] == b"PK\x03\x04" {
            index - 30 + 6
        } else {
            assert_eq!(&bytes[index - 46..index - 42], b"PK\x01\x02");
            index - 46 + 8
        };
        let flags = u16::from_le_bytes([bytes[flags_offset], bytes[flags_offset + 1]]);
        assert_eq!(flags & 0x0800, 0);
        assert_eq!(flags & 1, u16::from(encrypted));
        bytes[index + 3] = 0x82;
    }
}

fn payload(mut reader: impl Read) -> TestResult<Vec<u8>> {
    let mut actual = Vec::new();
    reader.read_to_end(&mut actual)?;
    Ok(actual)
}

#[test]
fn should_enumerate_decoded_cp437_name_and_read_indexed_payload() -> TestResult {
    let mut zip = archive(&[("cafe.txt", PAYLOAD)], None)?;
    assert_eq!(zip.len(), 1);
    assert_eq!(
        zip.file_names().collect::<Result<Vec<_>, _>>()?,
        ["café.txt"]
    );
    assert_eq!(payload(zip.by_index(0)?)?, PAYLOAD);
    Ok(())
}

#[test]
fn should_resolve_decoded_cp437_name_to_index() -> TestResult {
    let zip = archive(&[("cafe.txt", PAYLOAD)], None)?;
    assert_eq!(zip.index_for_name("café.txt"), Some(0));
    Ok(())
}

#[test]
fn should_read_cp437_member_by_decoded_name() -> TestResult {
    let mut zip = archive(&[("cafe.txt", PAYLOAD)], None)?;
    assert_eq!(payload(zip.by_name("café.txt")?)?, PAYLOAD);
    Ok(())
}

#[test]
fn should_read_cp437_member_with_options_by_decoded_name() -> TestResult {
    let mut zip = archive(&[("cafe.txt", PAYLOAD)], None)?;
    assert_eq!(
        payload(zip.by_name_with_options("café.txt", zip::ZipReadOptions::new())?)?,
        PAYLOAD
    );
    Ok(())
}

#[test]
fn should_read_cp437_member_by_decoded_path() -> TestResult {
    let mut zip = archive(&[("cafe.txt", PAYLOAD)], None)?;
    assert_eq!(payload(zip.by_path(Path::new("café.txt"))?)?, PAYLOAD);
    Ok(())
}

#[test]
fn should_seek_cp437_member_by_decoded_name() -> TestResult {
    let mut zip = archive(&[("cafe.txt", PAYLOAD)], None)?;
    let mut file = zip.by_name_seek("café.txt")?;
    file.seek(std::io::SeekFrom::Start(10))?;
    assert_eq!(payload(file)?, &PAYLOAD[10..]);
    Ok(())
}

#[test]
fn should_decrypt_cp437_member_by_decoded_name() -> TestResult {
    let password = fixture_password();
    let mut zip = archive(&[("cafe.txt", PAYLOAD)], Some(&password))?;
    assert_eq!(
        payload(zip.by_name_decrypt("café.txt", &password)?)?,
        PAYLOAD
    );
    Ok(())
}

#[test]
fn should_decrypt_cp437_member_by_decoded_path() -> TestResult {
    let password = fixture_password();
    let mut zip = archive(&[("cafe.txt", PAYLOAD)], Some(&password))?;
    assert_eq!(
        payload(zip.by_path_decrypt("café.txt", &password)?)?,
        PAYLOAD
    );
    Ok(())
}

#[test]
fn should_preserve_password_required_error_for_decoded_name() -> TestResult {
    let password = fixture_password();
    let mut zip = archive(&[("cafe.txt", PAYLOAD)], Some(&password))?;
    assert!(
        matches!(zip.by_name("café.txt"), Err(ZipError::UnsupportedArchive(message))
        if message == zip::result::ZipError::PASSWORD_REQUIRED)
    );
    Ok(())
}

#[test]
fn should_preserve_invalid_password_error_for_decoded_name() -> TestResult {
    let password = fixture_password();
    let mut zip = archive(&[("cafe.txt", PAYLOAD)], Some(&password))?;
    assert_eq!(payload(zip.by_index_decrypt(0, &password)?)?, PAYLOAD);
    let mut wrong_password = password;
    // ~keep ZipCrypto checks one header byte; indexed lookup selects a rejected password without name lookup.
    for difference in 1..=u8::MAX {
        wrong_password[0] = password[0] ^ difference;
        assert_ne!(wrong_password, password);
        let rejected = match zip.by_index_decrypt(0, &wrong_password) {
            Err(ZipError::InvalidPassword) => true,
            Ok(_) => false,
            Err(error) => return Err(error.into()),
        };
        if rejected {
            assert!(matches!(
                zip.by_name_decrypt("café.txt", &wrong_password),
                Err(ZipError::InvalidPassword)
            ));
            return Ok(());
        }
    }
    Err("No distinct password was rejected by indexed decryption".into())
}

#[test]
fn should_prefer_raw_utf8_match_over_earlier_cp437_decoded_collision() -> TestResult {
    let mut zip = archive(&[("cafe.txt", b"cp437"), ("café.txt", b"utf8")], None)?;
    assert_eq!(
        zip.file_names().collect::<Result<Vec<_>, _>>()?,
        ["café.txt", "café.txt"]
    );
    assert_eq!(zip.index_for_name("café.txt"), Some(1));
    assert_eq!(payload(zip.by_name("café.txt")?)?, b"utf8");
    Ok(())
}

#[test]
fn should_preserve_raw_utf8_match_before_later_cp437_decoded_collision() -> TestResult {
    let mut zip = archive(&[("café.txt", b"utf8"), ("cafe.txt", b"cp437")], None)?;
    assert_eq!(zip.index_for_name("café.txt"), Some(0));
    assert_eq!(payload(zip.by_name("café.txt")?)?, b"utf8");
    Ok(())
}

#[test]
fn should_keep_missing_name_and_path_not_found() -> TestResult {
    let password = fixture_password();
    let mut zip = archive(&[("cafe.txt", PAYLOAD)], None)?;
    assert_eq!(zip.index_for_name("missing.txt"), None);
    assert!(matches!(
        zip.by_name("missing.txt"),
        Err(ZipError::FileNotFound)
    ));
    assert!(matches!(
        zip.by_path("missing.txt"),
        Err(ZipError::FileNotFound)
    ));
    assert!(matches!(
        zip.by_name_decrypt("missing.txt", &password),
        Err(ZipError::FileNotFound)
    ));
    Ok(())
}

#[test]
fn should_not_normalize_separators_during_decoded_lookup() -> TestResult {
    let mut zip = archive(
        &[("cafe.txt", PAYLOAD), ("folder\\file.txt", b"backslash")],
        None,
    )?;
    assert_eq!(zip.index_for_name("folder/file.txt"), None);
    assert!(matches!(
        zip.by_name("folder/file.txt"),
        Err(ZipError::FileNotFound)
    ));
    assert_eq!(payload(zip.by_name("folder\\file.txt")?)?, b"backslash");
    Ok(())
}
