#[test]
fn test_prepared_file_roundtrip() {
    use std::io::{Cursor, Read, Write};
    use zip::ZipWriter;
    use zip::write::ZipFileBuilder;
    use zip::write::{FullFileOptions, SimpleFileOptions};

    let options = SimpleFileOptions::default();
    let mut builder = ZipFileBuilder::new(
        "prepared.txt",
        FullFileOptions::default().with_file_comment("file comment"),
    )
    .unwrap();
    builder.write_all(b"Contents of the prepared file").unwrap();
    let prepared = builder.finish().unwrap();
    let empty = ZipFileBuilder::new("empty.txt", options)
        .unwrap()
        .finish()
        .unwrap();

    // Interleave the prepared files with normally-written files.
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("before.txt", options).unwrap();
    writer.write_all(b"before").unwrap();
    writer.add_prepared_file(prepared).unwrap();
    writer.start_file("after.txt", options).unwrap();
    writer.write_all(b"after").unwrap();
    writer.add_prepared_file(empty).unwrap();

    let mut archive = writer.finish_into_readable().unwrap();
    let mut contents = String::new();
    for (name, expected) in [
        ("before.txt", "before"),
        ("prepared.txt", "Contents of the prepared file"),
        ("after.txt", "after"),
        ("empty.txt", ""),
    ] {
        contents.clear();
        archive
            .by_name(name)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        assert_eq!(contents, expected);
    }
    assert_eq!(
        archive.by_name("prepared.txt").unwrap().comment(),
        "file comment"
    );
}

#[test]
fn test_prepared_file_duplicate_name() {
    use std::io::{Cursor, Write};
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;
    use zip::write::ZipFileBuilder;

    let options = SimpleFileOptions::default();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));

    let mut builder = ZipFileBuilder::new("dup.txt", options).unwrap();
    builder.write_all(b"first").unwrap();
    writer.add_prepared_file(builder.finish().unwrap()).unwrap();

    let mut builder = ZipFileBuilder::new("dup.txt", options).unwrap();
    builder.write_all(b"second").unwrap();
    assert!(writer.add_prepared_file(builder.finish().unwrap()).is_err());

    // The writer must still be usable after the failed add.
    let mut builder = ZipFileBuilder::new("other.txt", options).unwrap();
    builder.write_all(b"third").unwrap();
    writer.add_prepared_file(builder.finish().unwrap()).unwrap();
    let archive = writer.finish_into_readable().unwrap();
    assert_eq!(archive.len(), 2);
}

#[test]
fn test_prepared_file_stream_mode() {
    use std::io::Read;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    use zip::write::ZipFileBuilder;
    use zip::{ZipArchive, ZipWriter};

    let options = SimpleFileOptions::default();
    let mut builder = ZipFileBuilder::new("streamed.txt", options).unwrap();
    builder.write_all(b"streamed contents").unwrap();
    let prepared = builder.finish().unwrap();

    let mut writer = ZipWriter::new_stream(Vec::new());
    writer.add_prepared_file(prepared).unwrap();
    let bytes = writer.finish().unwrap().into_inner();

    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut contents = String::new();
    archive
        .by_name("streamed.txt")
        .unwrap()
        .read_to_string(&mut contents)
        .unwrap();
    assert_eq!(contents, "streamed contents");
}

#[test]
fn test_prepared_file_rejects_encryption() {
    use zip::CompressionMethod::Stored;
    use zip::unstable::write::FileOptionsExt;
    use zip::write::SimpleFileOptions;
    use zip::write::ZipFileBuilder;

    let options = SimpleFileOptions::default()
        .compression_method(Stored)
        .with_deprecated_encryption(b"password")
        .unwrap();
    assert!(ZipFileBuilder::new("secret.txt", options).is_err());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_prepared_files_on_threads() {
    use std::io::{Cursor, Read, Write};
    use zip::ZipWriter;
    use zip::result::ZipResult;
    use zip::write::SimpleFileOptions;
    use zip::write::ZipFileBuilder;

    let entries: Vec<(String, Vec<u8>)> = (0..8)
        .map(|i| (format!("file{i}.bin"), vec![i as u8; 10_000]))
        .collect();

    let prepared: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = entries
            .iter()
            .map(|(name, data)| {
                scope.spawn(move || -> ZipResult<_> {
                    let mut builder = ZipFileBuilder::new(name, SimpleFileOptions::default())?;
                    builder.write_all(data)?;
                    builder.finish()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<ZipResult<Vec<_>>>()
    })
    .unwrap();

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for file in prepared {
        writer.add_prepared_file(file).unwrap();
    }
    let mut archive = writer.finish_into_readable().unwrap();
    let mut contents = Vec::new();
    for (name, data) in &entries {
        contents.clear();
        archive
            .by_name(name)
            .unwrap()
            .read_to_end(&mut contents)
            .unwrap();
        assert_eq!(&contents, data);
    }
}
