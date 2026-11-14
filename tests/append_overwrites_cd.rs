use std::io::{Cursor, Read, Seek, SeekFrom, Write};

use zip::read::{ZipFile, ZipFileEntry, read_zipfile_from_stream};
use zip::result::ZipResult;
use zip::unstable::stream::{ZipStreamReader, ZipStreamVisitor};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

fn count_signature(haystack: &[u8], signature: &[u8; 4]) -> usize {
    haystack
        .windows(signature.len())
        .filter(|window| *window == signature)
        .count()
}

fn read_all_via_stream(bytes: &[u8]) -> ZipResult<Vec<(String, Vec<u8>)>> {
    let mut cursor = Cursor::new(bytes);
    let mut entries = Vec::new();
    while let Some(mut file) = read_zipfile_from_stream(&mut cursor)? {
        let name = file.name()?.into_owned();
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;
        entries.push((name, content));
    }
    Ok(entries)
}

#[derive(Default)]
struct RecordingVisitor {
    local_entries: Vec<(String, Vec<u8>)>,
    central_names: Vec<String>,
}

impl ZipStreamVisitor for RecordingVisitor {
    fn visit_file<R: Read>(&mut self, file: &mut ZipFile<'_, R>) -> ZipResult<()> {
        let name = file.name()?.into_owned();
        let mut content = Vec::new();
        file.read_to_end(&mut content)?;
        self.local_entries.push((name, content));
        Ok(())
    }

    fn visit_additional_metadata(&mut self, metadata: &ZipFileEntry<'_>) -> ZipResult<()> {
        self.central_names.push(metadata.name()?.into_owned());
        Ok(())
    }
}

fn read_strict_stream(bytes: &[u8]) -> ZipResult<RecordingVisitor> {
    let mut visitor = RecordingVisitor::default();
    ZipStreamReader::new(Cursor::new(bytes))
        .enforce_header_consistency(true)
        .visit(&mut visitor)?;
    Ok(visitor)
}

#[test]
fn new_append_overwrites_central_directory() -> ZipResult<()> {
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("a.txt", stored)?;
    writer.write_all(b"alpha")?;
    writer.start_file("b.txt", stored)?;
    writer.write_all(b"beta")?;
    let buf = writer.finish()?.into_inner();

    let old_cd_start = {
        let archive = ZipArchive::new(Cursor::new(&buf))?;
        let cd_start = archive.central_directory_start();
        assert_eq!(
            &buf[cd_start as usize..cd_start as usize + 4],
            b"PK\x01\x02"
        );
        cd_start
    };

    let mut writer = ZipWriter::new_append(Cursor::new(buf))?;
    writer.start_file("c.txt", stored)?;
    writer.write_all(b"gamma")?;
    let buf = writer.finish()?.into_inner();

    assert_eq!(count_signature(&buf, b"PK\x01\x02"), 3);
    assert_eq!(count_signature(&buf, b"PK\x05\x06"), 1);

    let mut archive = ZipArchive::new(Cursor::new(&buf))?;
    assert_eq!(archive.len(), 3);
    assert!(!archive.has_overlapping_files()?);
    assert!(!archive.has_overlapping_entries()?);
    archive.validate_header_consistency()?;
    assert_eq!(archive.by_name("c.txt")?.header_start(), old_cd_start);

    for (name, expected) in [
        ("a.txt", b"alpha".as_slice()),
        ("b.txt", b"beta".as_slice()),
        ("c.txt", b"gamma".as_slice()),
    ] {
        let mut actual = Vec::new();
        archive.by_name(name)?.read_to_end(&mut actual)?;
        assert_eq!(actual, expected);
    }

    let expected_entries = vec![
        ("a.txt".to_owned(), b"alpha".to_vec()),
        ("b.txt".to_owned(), b"beta".to_vec()),
        ("c.txt".to_owned(), b"gamma".to_vec()),
    ];
    assert_eq!(read_all_via_stream(&buf)?, expected_entries);

    let visited = read_strict_stream(&buf)?;
    assert_eq!(visited.local_entries, expected_entries);
    assert_eq!(visited.central_names, ["a.txt", "b.txt", "c.txt"]);

    Ok(())
}

#[test]
fn repeated_new_append_leaves_only_one_central_directory() -> ZipResult<()> {
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("entry_0.txt", stored)?;
    writer.write_all(b"payload_0")?;
    let mut buf = writer.finish()?.into_inner();

    let mut expected = vec![("entry_0.txt".to_owned(), b"payload_0".to_vec())];

    for idx in 1..=3 {
        let old_cd_start = ZipArchive::new(Cursor::new(&buf))?.central_directory_start();
        let name = format!("entry_{idx}.txt");
        let payload = format!("payload_{idx}").into_bytes();

        let mut writer = ZipWriter::new_append(Cursor::new(buf))?;
        writer.start_file(&name, stored)?;
        writer.write_all(&payload)?;
        buf = writer.finish()?.into_inner();

        expected.push((name.clone(), payload));

        assert_eq!(count_signature(&buf, b"PK\x01\x02"), idx + 1);
        assert_eq!(count_signature(&buf, b"PK\x05\x06"), 1);

        let mut archive = ZipArchive::new(Cursor::new(&buf))?;
        assert_eq!(archive.len(), idx + 1);
        assert_eq!(archive.by_name(&name)?.header_start(), old_cd_start);
        assert!(!archive.has_overlapping_entries()?);
        archive.validate_header_consistency()?;
    }

    assert_eq!(read_all_via_stream(&buf)?, expected);
    assert_eq!(read_strict_stream(&buf)?.local_entries, expected);
    Ok(())
}

#[test]
fn new_append_preserves_prepended_data_and_overwrites_cd() -> ZipResult<()> {
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let prefix = b"PREFIX_JUNK_1234";

    let mut cursor = Cursor::new(prefix.to_vec());
    cursor.seek(SeekFrom::End(0))?;
    let mut writer = ZipWriter::new(cursor);
    writer.start_file("a.txt", stored)?;
    writer.write_all(b"first")?;
    let buf = writer.finish()?.into_inner();

    let old_cd_start = ZipArchive::new(Cursor::new(&buf))?.central_directory_start();
    assert!(old_cd_start > prefix.len() as u64);

    let mut writer = ZipWriter::new_append(Cursor::new(buf))?;
    writer.start_file("b.txt", stored)?;
    writer.write_all(b"second")?;
    let buf = writer.finish()?.into_inner();

    assert_eq!(&buf[..prefix.len()], prefix);
    assert_eq!(count_signature(&buf, b"PK\x01\x02"), 2);
    assert_eq!(count_signature(&buf, b"PK\x05\x06"), 1);

    let mut archive = ZipArchive::new(Cursor::new(&buf))?;
    assert_eq!(archive.len(), 2);
    assert_eq!(archive.by_name("b.txt")?.header_start(), old_cd_start);
    assert!(!archive.has_overlapping_files()?);
    archive.validate_header_consistency()?;

    for (name, expected) in [
        ("a.txt", b"first".as_slice()),
        ("b.txt", b"second".as_slice()),
    ] {
        let mut actual = Vec::new();
        archive.by_name(name)?.read_to_end(&mut actual)?;
        assert_eq!(actual, expected);
    }

    Ok(())
}

#[test]
fn new_append_on_empty_archive_overwrites_empty_eocd() -> ZipResult<()> {
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let buf = ZipWriter::new(Cursor::new(Vec::new()))
        .finish()?
        .into_inner();
    assert_eq!(buf.len(), 22);

    let mut writer = ZipWriter::new_append(Cursor::new(buf))?;
    writer.start_file("only.txt", stored)?;
    writer.write_all(b"solo")?;
    let buf = writer.finish()?.into_inner();

    assert_eq!(count_signature(&buf, b"PK\x01\x02"), 1);
    assert_eq!(count_signature(&buf, b"PK\x05\x06"), 1);

    let mut archive = ZipArchive::new(Cursor::new(&buf))?;
    assert_eq!(archive.len(), 1);
    assert_eq!(archive.by_name("only.txt")?.header_start(), 0);
    archive.validate_header_consistency()?;
    assert_eq!(
        read_all_via_stream(&buf)?,
        vec![("only.txt".to_owned(), b"solo".to_vec())]
    );
    assert_eq!(
        read_strict_stream(&buf)?.local_entries,
        vec![("only.txt".to_owned(), b"solo".to_vec())]
    );

    Ok(())
}

#[test]
fn new_append_with_shorter_comment_leaves_single_eocd() -> ZipResult<()> {
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.set_comment("x".repeat(128))?;
    writer.start_file("a.txt", stored)?;
    writer.write_all(b"alpha")?;
    writer.start_file("b.txt", stored)?;
    writer.write_all(b"beta")?;
    let buf = writer.finish()?.into_inner();

    let mut writer = ZipWriter::new_append(Cursor::new(buf))?;
    writer.set_comment("")?;
    let buf = writer.finish()?.into_inner();

    assert_eq!(count_signature(&buf, b"PK\x05\x06"), 1);
    assert_eq!(&buf[buf.len() - 22..buf.len() - 18], b"PK\x05\x06");

    let mut archive = ZipArchive::new(Cursor::new(&buf))?;
    assert_eq!(archive.len(), 2);
    assert_eq!(archive.comment(), b"");
    assert!(!archive.has_overlapping_entries()?);
    archive.validate_header_consistency()?;

    for (name, expected) in [
        ("a.txt", b"alpha".as_slice()),
        ("b.txt", b"beta".as_slice()),
    ] {
        let mut actual = Vec::new();
        archive.by_name(name)?.read_to_end(&mut actual)?;
        assert_eq!(actual, expected);
    }

    Ok(())
}
