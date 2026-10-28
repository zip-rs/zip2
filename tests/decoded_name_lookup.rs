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
        let mut password = [0u8; PASSWORD_LENGTH];
        getrandom::fill(&mut password).expect("Failed to generate random password");
        password
    }
}

fn archive_with_raw_names(entries: &[(&[u8], &[u8])], password: Option<&[u8]>) -> TestResult<ZipArchive<Cursor<Vec<u8>>>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let mut options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        if let Some(password) = password {
        options = options.with_deprecated_encryption(password)?;
    }
    for (name, contents) in entries {
        writer.start_file_with_options(options.name(name))?;
        writer.write_all(contents)?;
    }
    Ok(ZipArchive::new(Cursor::new(writer.finish()?.into_inner()))?)
}

fn payload(mut reader: impl Read) -> TestResult<Vec<u8>> {
    let mut actual = Vec::new();
    reader.read_to_end(&mut actual)?;
    Ok(actual)
}

#[test]
fn should_enumerate_decoded_cp437_name_and_read_indexed_payload() -> TestResult {
    let mut zip = archive_with_raw_names(&[(b"caf\x82.txt", PAYLOAD)], None)?;
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
    let zip = archive_with_raw_names(&[(b"caf\x82.txt", PAYLOAD)], None)?;
    assert_eq!(zip.index_for_name("café.txt"), Some(0));
    Ok(())
}

#[test]
fn should_read_cp437_member_by_decoded_name() -> TestResult {
    let mut zip = archive_with_raw_names(&[(b"caf\x82.txt", PAYLOAD)], None)?;
    assert_eq!(payload(zip.by_name("café.txt")?)?, PAYLOAD);
    Ok(())
}

#[test]
fn should_read_cp437_member_with_options_by_decoded_name() -> TestResult {
    let mut zip = archive_with_raw_names(&[(b"caf\x82.txt", PAYLOAD)], None)?;
    assert_eq!(
        payload(zip.by_name_with_options("café.txt", zip::ZipReadOptions::new())?)?,
        PAYLOAD
    );
    Ok(())
}

#[test]
fn should_read_cp437_member_by_decoded_path() -> TestResult {
    let mut zip = archive_with_raw_names(&[(b"caf\x82.txt", PAYLOAD)], None)?;
    assert_eq!(payload(zip.by_path(Path::new("café.txt"))?)?, PAYLOAD);
    Ok(())
}

#[test]
fn should_seek_cp437_member_by_decoded_name() -> TestResult {
    let mut zip = archive_with_raw_names(&[(b"caf\x82.txt", PAYLOAD)], None)?;
    let mut file = zip.by_name_seek("café.txt")?;
    file.seek(std::io::SeekFrom::Start(10))?;
    assert_eq!(payload(file)?, &PAYLOAD[10..]);
    Ok(())
}

#[test]
fn should_decrypt_cp437_member_by_decoded_name() -> TestResult {
    let password = fixture_password();
    let mut zip = archive_with_raw_names(&[(b"caf\x82.txt", PAYLOAD)], Some(&password))?;
    assert_eq!(
        payload(zip.by_name_decrypt("café.txt", &password)?)?,
        PAYLOAD
    );
    Ok(())
}

#[test]
fn should_decrypt_cp437_member_by_decoded_path() -> TestResult {
    let password = fixture_password();
    let mut zip = archive_with_raw_names(&[(b"caf\x82.txt", PAYLOAD)], Some(&password))?;
    assert_eq!(
        payload(zip.by_path_decrypt("café.txt", &password)?)?,
        PAYLOAD
    );
    Ok(())
}

#[test]
fn should_preserve_password_required_error_for_decoded_name() -> TestResult {
    let password = fixture_password();
    let mut zip = archive_with_raw_names(&[(b"caf\x82.txt", PAYLOAD)], Some(&password))?;
    assert!(
        matches!(zip.by_name("café.txt"), Err(ZipError::UnsupportedArchive(message))
        if message == ZipError::PASSWORD_REQUIRED)
    );
    Ok(())
}

#[test]
fn should_preserve_invalid_password_error_for_decoded_name() -> TestResult {
    let password = fixture_password();
    let mut zip = archive_with_raw_names(&[(b"caf\x82.txt", PAYLOAD)], Some(&password))?;
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
    let mut zip = archive_with_raw_names(&[(b"caf\x82.txt", b"cp437"), ("café.txt".as_bytes(), b"utf8")], None)?;
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
    let mut zip = archive_with_raw_names(&[("café.txt".as_bytes(), b"utf8"), (b"caf\x82.txt", b"cp437")], None)?;
    assert_eq!(zip.index_for_name("café.txt"), Some(0));
    assert_eq!(payload(zip.by_name("café.txt")?)?, b"utf8");
    Ok(())
}

#[test]
fn should_keep_missing_name_and_path_not_found() -> TestResult {
    let password = fixture_password();
    let mut zip = archive_with_raw_names(&[(b"caf\x82.txt", PAYLOAD)], None)?;
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
    let mut zip = archive_with_raw_names(
        &[(b"caf\x82.txt", PAYLOAD), (b"folder\\file.txt", b"backslash")],
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

#[test]
fn should_use_raw_name_index_with_by_index() -> TestResult {
    let mut zip = archive_with_raw_names(&[(b"caf\x82.txt", b"cp437")], None)?;
    let file_index = zip.index_for_name_raw(b"caf\x82.txt").unwrap();
    assert_eq!(payload(zip.by_index(file_index)?)?, b"cp437");
    Ok(())
}

#[test]
fn should_not_resolve_decoded_cp437_name_as_raw() -> TestResult {
    let zip = archive_with_raw_names(&[(b"caf\x82.txt", b"cp437")], None)?;
    assert_eq!(zip.index_for_name_raw("café.txt".as_bytes()), None);
    Ok(())
}

#[test]
#[allow(invalid_from_utf8)]
fn should_resolve_non_utf8_raw_name() -> TestResult {
    let zip = archive_with_raw_names(&[(b"caf\x82.txt", b"cp437")], None)?;
    let raw_name = b"caf\x82.txt";
    assert!(std::str::from_utf8(raw_name).is_err());
    assert_eq!(zip.index_for_name_raw(raw_name), Some(0));
    Ok(())
}

#[test]
fn should_prefer_raw_name_match_over_decoded_collision() -> TestResult {
    let mut zip =
        archive_with_raw_names(&[("café.txt".as_bytes(), b"utf8"), (b"caf\x82.txt", b"cp437")], None)?;
    let file_index = zip.index_for_name_raw(b"caf\x82.txt").unwrap();
    assert_eq!(file_index, 1);
    assert_eq!(payload(zip.by_index(file_index)?)?, b"cp437");
    Ok(())
}
