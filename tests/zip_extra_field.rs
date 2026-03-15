use std::io;
use zip::ZipArchive;

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
