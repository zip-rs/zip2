use std::io;
use zip::{ZipArchive, write::FileOptions};

fn generate_file_with_padding(padding_local_header: u16, padding_central_header: u16) -> Vec<u8> {
    let local_header = [
        0x50, 0x4B, 0x03, 0x04, // sig
        0x0A, 0x00, // version
        0x00, 0x00, //bits
        0x00, 0x00, // compression
        0xCD, 0x4B, // last mod
        0xA2, 0x58, // last mod
        0x00, 0x00, 0x00, 0x00, // crc
        0x00, 0x00, 0x00, 0x00, // size
        0x00, 0x00, 0x00, 0x00, // size
        0x08, 0x00, // filename size
    ];
    let padding_local_bytes = padding_local_header.to_le_bytes();
    let filename = b"test.txt"; // filename
    let padding_local = vec![0; padding_local_header as usize];
    let central_dir = [
        0x50, 0x4B, 0x01, 0x02, // sig Central directory header
        0x1E, // spec
        0x03, // os
        0x0A, // zip
        0x00, // os
        0x00, 0x00, // general flags
        0x00, 0x00, // compression
        0xCD, 0x4B, 0xA2, 0x58, // mod time
        0x00, 0x00, 0x00, 0x00, // crc
        0x00, 0x00, 0x00, 0x00, //size
        0x00, 0x00, 0x00, 0x00, // size
        0x08, 0x00, // filename len
    ];
    let padding_central_bytes = padding_central_header.to_le_bytes();
    let central_header_part_2 = [
        0x00, 0x00, // file comment length
        0x00, 0x00, // disk start
        0x00, 0x00, // internal file attributes
        0x00, 0x00, 0x00, 0x00, // external file attributes
        0x00, 0x00, 0x00, 0x00, // local header offset
    ];
    // important - filename is here
    let padding_central = vec![0; padding_central_header as usize];
    let rest = [
        0x50, 0x4B, 0x05, 0x06, // END CENTRAL HEADER
        0x00, 0x00, // number of this disk
        0x00, 0x00, // central dir disk
        0x01, 0x00, // entries on disk
        0x01, 0x00, // total entries
    ];
    let size_central_dir = (0x4B + padding_central_header) as u32;
    let size_central_dir_bytes = size_central_dir.to_le_bytes();
    let offset = (0x26 + padding_local_header) as u32;
    let offset_bytes = offset.to_le_bytes();
    let comment_length = [
        0x00, 0x00, // comment length
    ];
    let mut zip_file = Vec::new();
    zip_file.extend(local_header);
    zip_file.extend(padding_local_bytes);
    zip_file.extend(filename);
    zip_file.extend(padding_local);
    zip_file.extend(central_dir);
    zip_file.extend(padding_central_bytes);
    zip_file.extend(central_header_part_2);
    zip_file.extend(filename); // important
    zip_file.extend(padding_central);
    zip_file.extend(rest);
    zip_file.extend(size_central_dir_bytes);
    zip_file.extend(offset_bytes);
    zip_file.extend(comment_length);
    zip_file
}

#[test]
fn test_padding_in_extra_field() {
    let tests: Vec<(u16, u16)> = (0..=4).flat_map(|x| (0..=4).map(move |y| (x, y))).collect();
    for (local, central) in tests {
        let zip_file = generate_file_with_padding(local, central);

        // uncomment for debug
        // let filename = format!("tests/data/zip_extra_field_padding_double_{local}_{central}.zip",);
        // let mut file = File::create(filename).unwrap();
        // file.write_all(&zip_file).unwrap();

        let mut archive = ZipArchive::new(io::Cursor::new(&zip_file))
            .map_err(|e| format!("Padding is ({local} {central}). Error: {e}"))
            .expect("couldn't open test zip file");

        assert_eq!(archive.len(), 1);
        let file_text = archive.by_name("test.txt");
        assert!(
            file_text.is_ok(),
            "Cannot access test.txt for ({local} {central})"
        );
    }
}

#[test]
fn test_crc32_extra_field_name() {
    use std::io::Cursor;
    use zip::ZipArchive;

    let bytes = [
        0x50, 0x4B, 0x03, 0x04, 0x14, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20,
        0x30, 0x3A, 0x36, 0x06, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x09, 0x00, 0x12, 0x00,
        0x63, 0x61, 0x66, 0xC3, 0xA9, 0x2E, 0x74, 0x78, 0x74, // Unicode Path
        0x75, 0x70, 0x0E, 0x00, 0x01, 0x8F, 0x6E, 0x97, 0xA0, 0x63, 0x61, 0x66, 0xC3, 0xA9, 0x2E,
        0x74, 0x78, 0x74, 0x68, 0x65, 0x6C, 0x6C, 0x6F, 0x0A, 0x50, 0x4B, 0x01, 0x02, 0x14, 0x00,
        0x14, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x30, 0x3A, 0x36, 0x06,
        0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x09, 0x00, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x63, 0x61, 0x66, 0xC3, 0xA9,
        0x2E, 0x74, 0x78, 0x74, // Unicode Path
        0x75, 0x70, 0x0E, 0x00, 0x01, 0x8F, 0x6E, 0x97, 0xA0, 0x63, 0x61, 0x66, 0xC3, 0xA9, 0x2E,
        0x74, 0x78, 0x74, 0x50, 0x4B, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00,
        0x49, 0x00, 0x00, 0x00, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    let archive = ZipArchive::new(Cursor::new(bytes));
    assert!(archive.is_ok());
    archive.unwrap();
}

#[test]
fn test_crc32_extra_field_comment() {
    use std::io::Cursor;
    use zip::ZipArchive;

    let bytes = [
        0x50, 0x4B, 0x03, 0x04, 0x14, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20,
        0x30, 0x3A, 0x36, 0x06, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x09, 0x00, 0x12, 0x00,
        0x63, 0x61, 0x66, 0xC3, 0xA9, 0x2E, 0x74, 0x78, 0x74, // Unicode Comment
        0x75, 0x63, 0x0E, 0x00, 0x01, 0x8F, 0x6E, 0x97, 0xA0, 0x63, 0x61, 0x66, 0xC3, 0xA9, 0x2E,
        0x74, 0x78, 0x74, 0x68, 0x65, 0x6C, 0x6C, 0x6F, 0x0A, 0x50, 0x4B, 0x01, 0x02, 0x14, 0x00,
        0x14, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x30, 0x3A, 0x36, 0x06,
        0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x09, 0x00, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x63, 0x61, 0x66, 0xC3, 0xA9,
        0x2E, 0x74, 0x78, 0x74, // Unicode Comment
        0x75, 0x63, 0x0E, 0x00, 0x01, 0x8F, 0x6E, 0x97, 0xA0, 0x63, 0x61, 0x66, 0xC3, 0xA9, 0x2E,
        0x74, 0x78, 0x74, 0x50, 0x4B, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00,
        0x49, 0x00, 0x00, 0x00, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    let archive = ZipArchive::new(Cursor::new(bytes));
    assert!(archive.is_ok());
    archive.unwrap();
}

#[test]
fn test_extra_field_too_long() {
    use std::io::Cursor;
    use zip::ZipWriter;
    // u16::MAX = 65535
    // Size of extra field header and length = 4
    // Magic = 4
    // ZipLocalHeader = 26
    // filename = 1
    let tests = [
        // should NOT fail since value is less than u16::MAX
        (vec![1; 65535 - 4 - 4 - 26 - 1 - 1], false),
        // should fail since value is exactly u16::MAX
        (vec![1; 65535 - 4 - 4 - 26 - 1], true),
        // should fail since value is more than u16::MAX
        (vec![1; 65535 - 4 - 4 - 26], true),
    ];
    for (extra_field, should_fail) in tests {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.set_flush_on_finish_file(false);
        let mut options = FileOptions::default();
        eprintln!(
            "Extra_field_len = {}, total = {}",
            extra_field.len(),
            extra_field.len() + 4 + 4 + 26 + 1
        );
        options.add_extra_field(0x1e51, extra_field, true).unwrap();
        if should_fail {
            writer.start_file("a", options).unwrap_err();
        } else {
            writer.start_file("a", options).unwrap();
        }
    }
}

#[test]
fn test_alignment_extra_field_local_only() {
    use std::io::{Cursor, Write};
    use zip::CompressionMethod;
    use zip::ZipArchive;
    use zip::write::{SimpleFileOptions, ZipWriter};

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .with_alignment(16);
    writer.start_file("test.txt", options).unwrap();
    writer.write_all(b"hello world").unwrap();
    let zip_bytes = writer.finish().unwrap().into_inner();

    // Verify 0xa11e alignment tag is present in the local header bytes
    let tag = (0xa11e_u16).to_le_bytes();
    assert_eq!(
        zip_bytes.windows(2).filter(|w| *w == tag).count(),
        1,
        "Local header must contain alignment extra field tag 0xa11e"
    );

    // Central directory should not contain DataStreamAlignment
    let mut archive = ZipArchive::new(Cursor::new(&zip_bytes)).unwrap();
    let file = archive.by_name("test.txt").unwrap();
    let has_alignment_central = file.extra_data_fields().any(|ef| {
        matches!(
            ef,
            zip::extra_fields::ExtraField::DataStreamAlignment { .. }
        )
    });
    assert!(
        !has_alignment_central,
        "Central directory header must NOT contain DataStreamAlignment"
    );
}

#[test]
fn test_custom_central_only_extra_field() {
    use std::io::{Cursor, Write};
    use zip::CompressionMethod;
    use zip::ZipArchive;
    use zip::extra_fields::ExtraField;
    use zip::write::{FileOptions, ZipWriter};

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let mut options = FileOptions::default().compression_method(CompressionMethod::Stored);
    options
        .add_extra_field(0x1234, vec![0xAB, 0xCD], true)
        .unwrap(); // central_only = true

    writer.start_file("central_only.txt", options).unwrap();
    writer.write_all(b"content").unwrap();
    let bytes = writer.finish().unwrap().into_inner();

    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let file = archive.by_name("central_only.txt").unwrap();

    // Central directory should have this field
    let has_custom_central = file.extra_data_fields().any(|ef| match ef {
        ExtraField::Custom(cef) => cef.header_id == 0x1234 && *cef.data == [0xAB, 0xCD],
        _ => false,
    });
    assert!(
        has_custom_central,
        "Central directory header must contain central_only custom field"
    );
}
