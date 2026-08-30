use std::collections::HashSet;
use std::io::{Cursor, Read, Seek, Write};
use zip::result::ZipResult;
use zip::write::ExtendedFileOptions;
use zip::write::FileOptions;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, SUPPORTED_COMPRESSION_METHODS, ZipWriter};

fn for_each_supported_method<F>(mut f: F)
where
    F: FnMut(CompressionMethod),
{
    for &method in SUPPORTED_COMPRESSION_METHODS {
        if method == CompressionMethod::DEFLATE
            && cfg!(all(
                feature = "deflate-zopfli",
                not(feature = "deflate-flate2")
            ))
        {
            // We do not support DEFLATE decompression without the `flate2` feature.
            continue;
        }

        if method == CompressionMethod::DEFLATE64 {
            continue;
        }

        f(method);
    }
}

// This test asserts that after creating a zip file, then reading its contents back out,
// the extracted data will *always* be exactly the same as the original data.
#[test]
pub fn end_to_end() {
    for_each_supported_method(|method| {
        let file = &mut Cursor::new(Vec::new());

        write_test_archive(file, method, true);

        check_archive_file(file, ENTRY_NAME, Some(method), LOREM_IPSUM);
        check_archive_file(file, INTERNAL_COPY_ENTRY_NAME, Some(method), LOREM_IPSUM);
    });
}

// This test asserts that after copying a `ZipFile` to a new `ZipWriter`, then reading its
// contents back out, the extracted data will *always* be exactly the same as the original data.
#[test]
fn test_copy_zip_entries() {
    for_each_supported_method(|method| {
        let src_file = &mut Cursor::new(Vec::new());
        write_test_archive(src_file, method, false);

        let mut tgt_file = Cursor::new(Vec::new());

        {
            let mut src_archive = zip::ZipArchive::new(src_file).unwrap();
            let mut zip = ZipWriter::new(&mut tgt_file);

            {
                let file = src_archive
                    .by_name(ENTRY_NAME)
                    .expect("Missing expected file");

                zip.raw_copy_file(file).expect("Couldn't copy file");
            }

            {
                let file = src_archive
                    .by_name(ENTRY_NAME)
                    .expect("Missing expected file");

                zip.raw_copy_file_rename(file, COPY_ENTRY_NAME)
                    .expect("Couldn't copy and rename file");
            }
        }

        let mut tgt_archive = zip::ZipArchive::new(&mut tgt_file).unwrap();

        check_archive_file_contents(&mut tgt_archive, ENTRY_NAME, LOREM_IPSUM);
        check_archive_file_contents(&mut tgt_archive, COPY_ENTRY_NAME, LOREM_IPSUM);
    });
}

#[test]
fn test_copy_zip_symlink() {
    const LINK_NAME: &str = "symlink";
    const LINK_TARGET: &str = "link-target";
    for_each_supported_method(|method| {
        let mut src_archive = {
            let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            zip.add_symlink_from_path(
                LINK_NAME,
                LINK_TARGET,
                zip::write::SimpleFileOptions::DEFAULT.compression_method(method),
            )
            .unwrap();
            zip.finish_into_readable().unwrap()
        };
        let mut tgt_archive = {
            let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            zip.raw_copy_file(src_archive.by_name(LINK_NAME).unwrap())
                .unwrap();
            zip.finish_into_readable().unwrap()
        };

        let src_file = src_archive.by_name(LINK_NAME).unwrap();
        let mut tgt_file = tgt_archive.by_name(LINK_NAME).unwrap();

        {
            let mut dest_buf = String::new();
            tgt_file.read_to_string(&mut dest_buf).unwrap();
            assert_eq!(dest_buf.as_str(), LINK_TARGET);
        }
        assert_eq!(src_file.compression(), tgt_file.compression());
        assert_eq!(src_file.unix_mode(), tgt_file.unix_mode());
    });
}

// This test asserts that after appending to a `ZipWriter`, then reading its contents back out,
// both the prior data and the appended data will be exactly the same as their originals.
#[test]
fn test_append_to_zip() {
    for_each_supported_method(|method| {
        for shallow_copy in &[false, true] {
            let mut file = Cursor::new(Vec::new());
            write_test_archive(&mut file, method, *shallow_copy);

            {
                let mut zip = ZipWriter::new_append(&mut file).unwrap();
                zip.start_file(
                    COPY_ENTRY_NAME,
                    SimpleFileOptions::default()
                        .compression_method(method)
                        .unix_permissions(0o755),
                )
                .unwrap();
                zip.write_all(LOREM_IPSUM).unwrap();
                zip.finish().unwrap();
            }

            let mut zip = zip::ZipArchive::new(&mut file).unwrap();
            check_archive_file_contents(&mut zip, ENTRY_NAME, LOREM_IPSUM);
            check_archive_file_contents(&mut zip, COPY_ENTRY_NAME, LOREM_IPSUM);
            check_archive_file_contents(&mut zip, INTERNAL_COPY_ENTRY_NAME, LOREM_IPSUM);
        }
    });
}

// Write a test zip archive to buffer.
fn write_test_archive(file: &mut Cursor<Vec<u8>>, method: CompressionMethod, shallow_copy: bool) {
    let mut zip = ZipWriter::new(file);

    zip.add_directory("test/", SimpleFileOptions::default())
        .unwrap();

    let mut options = FileOptions::<ExtendedFileOptions>::default()
        .compression_method(method)
        .unix_permissions(0o755);

    zip.start_file(ENTRY_NAME, options.clone()).unwrap();
    zip.write_all(LOREM_IPSUM).unwrap();

    if shallow_copy {
        zip.shallow_copy_file(ENTRY_NAME, INTERNAL_COPY_ENTRY_NAME)
            .unwrap();
    } else {
        zip.deep_copy_file(ENTRY_NAME, INTERNAL_COPY_ENTRY_NAME)
            .unwrap();
    }

    zip.start_file("test/☃.txt", options.clone()).unwrap();
    zip.write_all(b"Hello, World!\n").unwrap();

    options
        .add_extra_field(0xbeef, EXTRA_DATA.to_owned().into_boxed_slice(), false)
        .unwrap();

    zip.start_file("test_with_extra_data/🐢.txt", options)
        .unwrap();
    zip.write_all(b"Hello, World! Again.\n").unwrap();

    zip.finish().unwrap();
}

// Load an archive from buffer and check for test data.
fn check_test_archive<R: Read + Seek>(zip_file: R) -> ZipResult<zip::ZipArchive<R>> {
    let mut archive = zip::ZipArchive::new(zip_file)?;

    // Check archive contains expected file names.
    {
        let expected_file_names = [
            "test/",
            "test/☃.txt",
            "test_with_extra_data/🐢.txt",
            ENTRY_NAME,
            INTERNAL_COPY_ENTRY_NAME,
        ];
        let expected_file_names: HashSet<String> =
            expected_file_names.iter().map(|f| f.to_string()).collect();
        let file_names: HashSet<String> = archive
            .file_names()
            .map(|f| f.unwrap().into_owned())
            .collect();
        assert_eq!(file_names, expected_file_names);
    }
    {
        // Check an archive file for extra data field contents.
        let file_without_extra_data = archive.by_name("test/☃.txt")?;
        assert_eq!(file_without_extra_data.extra_data(), None);
    }
    {
        // Check an archive file for extra data field contents.
        let file_with_extra_data = archive.by_name("test_with_extra_data/🐢.txt")?;
        let mut extra_field = Vec::new();
        extra_field.write_all(&0xbeef_u16.to_le_bytes())?;
        extra_field.write_all(&(EXTRA_DATA.len() as u16).to_le_bytes())?;
        extra_field.write_all(EXTRA_DATA)?;
        assert_eq!(file_with_extra_data.extra_data(), Some(extra_field));
    }

    Ok(archive)
}

// Read a file in the archive as a string.
fn read_archive_file<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> ZipResult<String> {
    let mut file = archive.by_name(name)?;

    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    Ok(contents)
}

// Check a file in the archive contains expected data and properties.
fn check_archive_file(
    zip_file: &mut Cursor<Vec<u8>>,
    name: &str,
    expected_method: Option<CompressionMethod>,
    expected_data: &[u8],
) {
    let mut archive = check_test_archive(zip_file).unwrap();

    if let Some(expected_method) = expected_method {
        // Check the file's compression method.
        let file = archive.by_name(name).unwrap();
        let real_method = file.compression();

        assert_eq!(
            expected_method, real_method,
            "File does not have expected compression method"
        );
    }

    check_archive_file_contents(&mut archive, name, expected_data);
}

// Check a file in the archive contains the given data.
fn check_archive_file_contents<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    expected: &[u8],
) {
    let file_permissions: u32 = archive.by_name(name).unwrap().unix_mode().unwrap();
    assert_eq!(file_permissions, EXPECTED_FILE_PERMISSIONS);

    let file_contents: String = read_archive_file(archive, name).unwrap();
    assert_eq!(file_contents.as_bytes(), expected);
}

const LOREM_IPSUM: &[u8] = br#"Lorem ipsum dolor sit amet, consectetur adipiscing elit. In tellus elit, tristique vitae mattis egestas, ultricies vitae risus. Quisque sit amet quam ut urna aliquet
molestie. Proin blandit ornare dui, a tempor nisl accumsan in. Praesent a consequat felis. Morbi metus diam, auctor in auctor vel, feugiat id odio. Curabitur ex ex,
dictum quis auctor quis, suscipit id lorem. Aliquam vestibulum dolor nec enim vehicula, porta tristique augue tincidunt. Vivamus ut gravida est. Sed pellentesque, dolor
vitae tristique consectetur, neque lectus pulvinar dui, sed feugiat purus diam id lectus. Class aptent taciti sociosqu ad litora torquent per conubia nostra, per
inceptos himenaeos. Maecenas feugiat velit in ex ultrices scelerisque id id neque.
"#;

const EXTRA_DATA: &[u8] = b"Extra Data";

const ENTRY_NAME: &str = "test/lorem_ipsum.txt";

const COPY_ENTRY_NAME: &str = "test/lorem_ipsum_renamed.txt";

const INTERNAL_COPY_ENTRY_NAME: &str = "test/lorem_ipsum_copied.txt";

#[cfg(windows)]
const EXPECTED_FILE_PERMISSIONS: u32 = 0o100664;
#[cfg(not(windows))]
const EXPECTED_FILE_PERMISSIONS: u32 = 0o100755;

#[test]
fn test_extra_field_mapping_contains_expected_values() {
    // just a test to access the variable in the crate
    use zip::extra_fields::EXTRA_FIELD_MAPPING;
    assert!(EXTRA_FIELD_MAPPING.is_sorted());

    // The following assertions ensure required extra field IDs are present.
    // ZIP64 extended information extra field - 0x0001 which is 1
    assert!(EXTRA_FIELD_MAPPING.contains(&0x0001));

    // Strong Encryption Header - 0x0017 which is 23
    assert!(EXTRA_FIELD_MAPPING.contains(&0x0017));

    // Additional checks for other well-known extra field IDs
    // Extended Timestamp - 0x5455
    assert!(EXTRA_FIELD_MAPPING.contains(&0x5455));

    // Info-ZIP Unix (UID/GID) - 0x7875
    assert!(EXTRA_FIELD_MAPPING.contains(&0x7875));
}

#[test]
fn test_long_comment_is_cut() {
    use std::io::{Cursor, Write};
    use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

    let comment_length = (u16::MAX as usize) + 100; // the comment is larger than the max
    let data = Vec::new();
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(true);
    let mut bytes = vec![0u8; comment_length];
    getrandom::fill(&mut bytes)
        .map_err(|e| std::io::Error::other(format!("getrandom error: {}", e)))
        .unwrap();

    let mut writer = ZipWriter::new(Cursor::new(data));
    let res = writer.set_raw_comment(bytes.clone().into_boxed_slice());
    assert!(res.is_err()); // `set_raw_comment` will throw an error
    writer.start_file("asdf.txt", options).unwrap();
    writer.write_all(b"asdf").unwrap();
    let archive_as_bytes = writer.finish().unwrap().into_inner();

    // reading
    let zip_reader = ZipArchive::new(Cursor::new(archive_as_bytes)).unwrap();
    let comment = zip_reader.comment();

    assert_eq!(comment.len(), u16::MAX as usize);
    assert_eq!(comment, &bytes[..(u16::MAX as usize)]);
}

// Test to use the HasZipMetadata trait which use a private unnamed type
#[test]
fn test_explicit_system_roundtrip() {
    use std::io::Cursor;
    use std::io::Write;
    use zip::CompressionMethod::Stored;
    use zip::HasZipMetadata; // We use the trait here
    use zip::System;
    use zip::ZipArchive;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;
    let system = System::Unix;

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(Stored)
        .system(system);

    let filename = format!("test_{:?}.txt", system);
    writer.start_file(&filename, options).unwrap();
    writer.write_all(b"content").unwrap();

    // Write and read back
    let bytes = writer.finish().unwrap().into_inner();
    let mut reader = ZipArchive::new(Cursor::new(bytes)).unwrap();

    let file = reader.by_index(0).unwrap();
    assert_eq!(
        file.get_metadata().system, // We use the trait here
        system,
        "System mismatch for {:?}",
        system
    );
}

/// Only on little endian because it runs too long with Miri CI
#[cfg(all(target_endian = "little", not(miri)))]
#[test]
fn test_64k_files() -> zip::result::ZipResult<()> {
    use std::io::{Read, Write};
    use zip::CompressionMethod;
    use zip::ZipArchive;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for i in 0..=u16::MAX {
        let file_name = format!("{i}.txt");
        writer.start_file(&*file_name, options)?;
        writer.write_all(i.to_string().as_bytes())?;
    }

    let mut reader = ZipArchive::new(writer.finish()?)?;
    for i in 0..=u16::MAX {
        let expected_name = format!("{i}.txt");
        let expected_contents = i.to_string();
        let expected_contents = expected_contents.as_bytes();
        let mut file = reader.by_name(&expected_name)?;
        let mut contents = Vec::with_capacity(expected_contents.len());
        file.read_to_end(&mut contents)?;
        assert_eq!(contents, expected_contents);
        drop(file);
        contents.clear();
        let mut file = reader.by_index(i as usize)?;
        file.read_to_end(&mut contents)?;
        assert_eq!(contents, expected_contents);
    }
    Ok(())
}

/// Only on little endian because we cannot use fs with miri CI
#[cfg(all(target_endian = "little", not(miri)))]
#[test]
fn test_can_create_destination() -> zip::result::ZipResult<()> {
    use tempfile::TempDir;
    use zip::ZipArchive;

    let mut reader = ZipArchive::new(Cursor::new(include_bytes!("../tests/data/mimetype.zip")))?;
    let dest = TempDir::with_prefix("read__test_can_create_destination")?;
    reader.extract(&dest)?;
    assert!(dest.path().join("mimetype").exists());
    Ok(())
}

#[test]
fn test_zip_file_entry() {
    use std::io::Write;
    use zip::CompressionMethod;
    use zip::ZipArchive;
    use zip::ZipWriter;
    use zip::read::ZipFileEntry;
    use zip::write::SimpleFileOptions;

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.start_file("my_file.txt", options).unwrap();
    writer.write_all(b"data").unwrap();

    let zip_archive = ZipArchive::new(writer.finish().unwrap()).unwrap();

    let file_number = zip_archive.index_for_name("my_file.txt").unwrap();
    let file = zip_archive.by_index_data(file_number).unwrap();
    assert_eq!(file.compression(), CompressionMethod::Stored);

    // we can use it in a callback
    let verify_file = |file: &ZipFileEntry| -> bool { file.size() > 2 };
    let is_correct_size = verify_file(&file);
    assert!(is_correct_size);

    // we can use it in a function
    fn verify_file_2(file: &ZipFileEntry) -> bool {
        file.name().unwrap() == "my_file.txt"
    }

    let is_correct_file_name = verify_file_2(&file);
    assert!(is_correct_file_name);
}

#[test]
fn test_zip_file_entry_to_reader() {
    use std::io::Write;
    use zip::CompressionMethod;
    use zip::ZipArchive;
    use zip::ZipReadOptions;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.start_file("my_file.txt", options).unwrap();
    writer.write_all(b"data").unwrap();

    let zip_raw = writer.finish().unwrap().into_inner().into_boxed_slice();

    let reader_zip = Cursor::new(&zip_raw);

    let zip_archive = ZipArchive::new(reader_zip).unwrap();

    let file_number = zip_archive.index_for_name("my_file.txt").unwrap();
    let file = zip_archive.by_index_data(file_number).unwrap();
    assert_eq!(file.compression(), CompressionMethod::Stored);

    let mut reader_zipfile = Cursor::new(&zip_raw);

    let mut file_with_reader = file
        .with_reader(&mut reader_zipfile, ZipReadOptions::new())
        .unwrap();

    let mut content = Vec::new();
    file_with_reader.read_to_end(&mut content).unwrap();
    assert_eq!(content, b"data");
}

#[test]
fn modify_in_place_with_trait() {
    use zip::HasZipMetadata;
    use zip::ZipArchive;
    // With the trait, the archive needs to be mut

    // create a zip
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let datetime = (23825, 44746).try_into().unwrap();
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(datetime);
    writer.start_file("my_file.txt", options).unwrap();
    writer.write_all(b"data").unwrap();

    let mut zip_raw = writer.finish().unwrap().into_inner();

    {
        // check current date
        let reader_zip = Cursor::new(&zip_raw);
        let mut zip_archive = ZipArchive::new(reader_zip).unwrap();
        let file = zip_archive.by_name("my_file.txt").unwrap();
        let date = file.get_metadata().last_modified_time.unwrap();
        assert_eq!(date.datepart(), 23825);
        assert_eq!(date.timepart(), 44746);
    }

    let start = {
        let reader_zip = Cursor::new(&zip_raw);
        let mut zip_archive = ZipArchive::new(reader_zip).unwrap();
        let file = zip_archive.by_name("my_file.txt").unwrap();
        file.get_metadata().central_header_start as usize
    };

    let offset_date = start + 14;
    zip_raw[offset_date] += 10;

    {
        // check new date
        let reader_zip = Cursor::new(&zip_raw);
        let mut zip_archive = ZipArchive::new(reader_zip).unwrap();
        let file = zip_archive.by_name("my_file.txt").unwrap();
        let date = file.get_metadata().last_modified_time.unwrap();
        assert_eq!(date.datepart(), 23835); // changed!
        assert_eq!(date.timepart(), 44746);
    }
}

#[test]
fn modify_in_place_with_data() {
    use zip::ZipArchive;
    // Without the trait, the archive doesn't need to be mut

    // create a zip
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let datetime = (23825, 44746).try_into().unwrap();
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(datetime);
    writer.start_file("my_file.txt", options).unwrap();
    writer.write_all(b"data").unwrap();

    let mut zip_raw = writer.finish().unwrap().into_inner();

    {
        // check current date
        let reader_zip = Cursor::new(&zip_raw);
        let zip_archive = ZipArchive::new(reader_zip).unwrap();
        let file_number = zip_archive.index_for_path("my_file.txt").unwrap();
        let file = zip_archive.by_index_data(file_number).unwrap();
        let date = file.last_modified().unwrap();
        assert_eq!(date.datepart(), 23825);
        assert_eq!(date.timepart(), 44746);
    }

    let start = {
        let reader_zip = Cursor::new(&zip_raw);
        let mut zip_archive = ZipArchive::new(reader_zip).unwrap();
        let file = zip_archive.by_name("my_file.txt").unwrap();
        file.central_header_start() as usize
    };

    let offset_date = start + 14;
    zip_raw[offset_date] += 10;

    {
        // check new date
        let reader_zip = Cursor::new(&zip_raw);
        let zip_archive = ZipArchive::new(reader_zip).unwrap();
        let file_number = zip_archive.index_for_path("my_file.txt").unwrap();
        let file = zip_archive.by_index_data(file_number).unwrap();
        let date = file.last_modified().unwrap();
        assert_eq!(date.datepart(), 23835); // changed!
        assert_eq!(date.timepart(), 44746);
    }
}
