use std::fs;
use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

fn archive(name: &str, content: &[u8]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(name, SimpleFileOptions::default())
        .unwrap();
    writer.write_all(content).unwrap();
    writer.finish().unwrap().into_inner()
}

fn relocate_offsets(bytes: &mut [u8], prefix_len: u32) {
    let central = bytes
        .windows(4)
        .position(|window| window == b"PK\x01\x02")
        .unwrap();
    let end = bytes
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .unwrap();

    let local_offset = u32::from_le_bytes(bytes[central + 42..central + 46].try_into().unwrap());
    bytes[central + 42..central + 46].copy_from_slice(&(local_offset + prefix_len).to_le_bytes());

    let central_offset = u32::from_le_bytes(bytes[end + 16..end + 20].try_into().unwrap());
    bytes[end + 16..end + 20].copy_from_slice(&(central_offset + prefix_len).to_le_bytes());
}

#[test]
fn two_concatenated_archives() -> Result<(), Box<dyn std::error::Error>> {
    let first = archive("plugins/bootstrap.js", b"console.log('ATTACKER_PLUGIN');\n");
    let mut second = archive(
        "documents/quarterly-report.pdf",
        b"%PDF-1.7\nbenign quarterly report\n",
    );
    relocate_offsets(&mut second, first.len() as u32);

    let mut combined = first;
    combined.extend_from_slice(&second);

    let mut strict_inspected = ZipArchive::new(Cursor::new(&combined))?;
    assert!(strict_inspected.validate_header_consistency().is_err());

    #[cfg(not(miri))]
    {
        use std::fs;
        use tempfile::tempdir;
        use zip::unstable::stream::ZipStreamReader;

        let destination = tempdir()?;
        let victim = destination.path().join("plugins/bootstrap.js");
        fs::create_dir_all(victim.parent().unwrap())?;
        fs::write(&victim, b"console.log('TRUSTED_PLUGIN');\n")?;

        let result = ZipStreamReader::new(Cursor::new(&combined))
            .enforce_header_consistency(true)
            .extract(destination.path());

        println!("strict stream extraction: {result:?}");
        println!(
            "victim after extraction: {:?}",
            fs::read_to_string(&victim)?
        );
        println!(
            "benign PDF extracted: {}",
            destination
                .path()
                .join("documents/quarterly-report.pdf")
                .exists()
        );

        assert!(result.is_err());
        assert!(!fs::read_to_string(&victim)?.contains("ATTACKER_PLUGIN"));
    }
    Ok(())
}

#[test]
fn second_archive_empty() -> Result<(), Box<dyn std::error::Error>> {
    let first = archive("plugins/bootstrap.js", b"console.log('ATTACKER_PLUGIN');\n");
    let second = ZipWriter::new(Cursor::new(Vec::new()));
    let second = second.finish()?.into_inner();

    let mut combined = first;
    combined.extend_from_slice(&second);

    let mut strict_inspected = ZipArchive::new(Cursor::new(&combined))?;
    assert!(strict_inspected.validate_header_consistency().is_err());

    #[cfg(not(miri))]
    {
        use tempfile::tempdir;
        use zip::unstable::stream::ZipStreamReader;

        let destination = tempdir()?;

        let result = ZipStreamReader::new(Cursor::new(&combined))
            .enforce_header_consistency(true)
            .extract(destination.path());

        assert!(result.is_err());
        assert!(
            matches!(fs::symlink_metadata(destination.path().join("plugins/bootstrap.js")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        );
    }
    Ok(())
}
