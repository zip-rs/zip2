use std::io::{Cursor, Read};
use zip::ZipArchive;

/// Entry 2 and Entry 3 have their Local File Header at offset 39, which is embedded
/// directly inside Entry 1's Stored data payload (bytes 39..70), so their data overlaps when their
/// size is greater than 0.
///
/// However, `has_overlapping_files()` doesn't count header-with-data overlaps such as the one
/// between Entry 1 and the other two, so it should return false when entries 2 and 3 are zero-size
/// and thus no byte is in the data of both.
#[test]
fn test_has_overlapping_files_zero_size_file_inside_other_file_data() {
    for inner_size in [0u32, 1] {
        let mut zip_bytes = Vec::new();

        let embedded_entry_data = vec![0; inner_size as usize];
        let inner_crc = crc32fast::hash(&embedded_entry_data);
        // A complete 31-byte Local File Header for an inner entry ("a")
        let embedded_entry: Vec<u8> = [
            &[
                0x50, 0x4b, 0x03, 0x04, // LFH signature (PK\x03\x04)
                0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0x00, 0x00, 0x00, 0x00, 0x00,
            ], // crc32 = 0
            inner_size.to_le_bytes().as_slice(), // compressed_size
            inner_size.to_le_bytes().as_slice(), // uncompressed_size
            &[
                0x01, 0x00, 0x00, 0x00, // file_name_length = 1, extra_field_length = 0
                b'a',
            ],
            &embedded_entry_data,
        ]
        .concat();
        let outer_crc = crc32fast::hash(&embedded_entry);
        let outer_name = b"outer.bin";

        // Entry 1 LFH at offset 0 (30 + 9 = 39 bytes; data payload is at 39..70)
        zip_bytes.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
        zip_bytes.extend_from_slice(&20u16.to_le_bytes());
        zip_bytes.extend_from_slice(&0u16.to_le_bytes());
        zip_bytes.extend_from_slice(&0u16.to_le_bytes()); // Stored
        zip_bytes.extend_from_slice(&0u16.to_le_bytes());
        zip_bytes.extend_from_slice(&0x21u16.to_le_bytes());
        zip_bytes.extend_from_slice(&outer_crc.to_le_bytes());
        zip_bytes.extend_from_slice(&(embedded_entry.len() as u32).to_le_bytes());
        zip_bytes.extend_from_slice(&(embedded_entry.len() as u32).to_le_bytes());
        zip_bytes.extend_from_slice(&(outer_name.len() as u16).to_le_bytes());
        zip_bytes.extend_from_slice(&0u16.to_le_bytes());
        zip_bytes.extend_from_slice(outer_name);

        let inner_lfh_offset = zip_bytes.len() as u32;
        assert_eq!(inner_lfh_offset, 39);
        zip_bytes.extend_from_slice(&embedded_entry);

        let cd_offset = zip_bytes.len() as u32;

        // Central Directory Entry 1: "outer.bin" -> local_header_offset = 0, compressed_size = 31
        zip_bytes.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
        zip_bytes.extend_from_slice(&20u16.to_le_bytes());
        zip_bytes.extend_from_slice(&20u16.to_le_bytes());
        zip_bytes.extend_from_slice(&0u16.to_le_bytes());
        zip_bytes.extend_from_slice(&0u16.to_le_bytes());
        zip_bytes.extend_from_slice(&0u16.to_le_bytes());
        zip_bytes.extend_from_slice(&0x21u16.to_le_bytes());
        zip_bytes.extend_from_slice(&outer_crc.to_le_bytes());
        zip_bytes.extend_from_slice(&(embedded_entry.len() as u32).to_le_bytes());
        zip_bytes.extend_from_slice(&(embedded_entry.len() as u32).to_le_bytes());
        zip_bytes.extend_from_slice(&(outer_name.len() as u16).to_le_bytes());
        zip_bytes.extend_from_slice(&[0u8; 12]);
        zip_bytes.extend_from_slice(&0u32.to_le_bytes());
        zip_bytes.extend_from_slice(outer_name);

        // Central Directory Entries 2 & 3: "inner_1" and "inner_2" both pointing to offset 39
        // (inside Entry 1's data payload!) with compressed_size = 0
        for name in [b"inner_1".as_slice(), b"inner_2".as_slice()] {
            zip_bytes.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
            zip_bytes.extend_from_slice(&20u16.to_le_bytes());
            zip_bytes.extend_from_slice(&20u16.to_le_bytes());
            zip_bytes.extend_from_slice(&0u16.to_le_bytes());
            zip_bytes.extend_from_slice(&0u16.to_le_bytes());
            zip_bytes.extend_from_slice(&0u16.to_le_bytes());
            zip_bytes.extend_from_slice(&0x21u16.to_le_bytes());
            zip_bytes.extend_from_slice(&inner_crc.to_le_bytes());
            zip_bytes.extend_from_slice(&inner_size.to_le_bytes());
            zip_bytes.extend_from_slice(&inner_size.to_le_bytes());
            zip_bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
            zip_bytes.extend_from_slice(&[0u8; 12]);
            zip_bytes.extend_from_slice(&inner_lfh_offset.to_le_bytes());
            zip_bytes.extend_from_slice(name);
        }

        let cd_size = (zip_bytes.len() as u32) - cd_offset;
        zip_bytes.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
        zip_bytes.extend_from_slice(&3u16.to_le_bytes());
        zip_bytes.extend_from_slice(&3u16.to_le_bytes());
        zip_bytes.extend_from_slice(&cd_size.to_le_bytes());
        zip_bytes.extend_from_slice(&cd_offset.to_le_bytes());
        zip_bytes.extend_from_slice(&0u16.to_le_bytes());

        let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();
        assert_eq!(archive.len(), 3);

        // All 3 overlapping entries extract cleanly without error:
        let mut buf = Vec::new();
        archive.by_index(0).unwrap().read_to_end(&mut buf).unwrap();
        assert_eq!(buf.len(), 31 + inner_size as usize);
        archive.by_index(1).unwrap().read_to_end(&mut buf).unwrap();
        assert_eq!(buf.len(), 31 + inner_size as usize * 2);
        archive.by_index(2).unwrap().read_to_end(&mut buf).unwrap();
        assert_eq!(buf.len(), 31 + inner_size as usize * 3);

        assert_eq!(inner_size != 0, archive.has_overlapping_files().unwrap());
        assert!(archive.has_overlapping_entries().unwrap());
    }
}

/// Entry 1's data payload is at 66..70 and Entry 2's data payload is at 70..74, so
/// `has_overlapping_files()` should return true.
#[test]
fn test_has_overlapping_files_content_of_first_is_header_of_second() {
    let mut zip_bytes = Vec::new();
    let data1 = b"AAAA";
    let data2 = b"BBBB";
    let crc1 = crc32fast::hash(data1);
    let crc2 = crc32fast::hash(data2);

    // LFH #1 at offset 0..35 (declares extra_len = 35 -> data_start_1 = 66, data_1 = 66..70)
    zip_bytes.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
    zip_bytes.extend_from_slice(&20u16.to_le_bytes());
    zip_bytes.extend_from_slice(&0u16.to_le_bytes());
    zip_bytes.extend_from_slice(&0u16.to_le_bytes());
    zip_bytes.extend_from_slice(&0u16.to_le_bytes());
    zip_bytes.extend_from_slice(&0x21u16.to_le_bytes());
    zip_bytes.extend_from_slice(&crc1.to_le_bytes());
    zip_bytes.extend_from_slice(&4u32.to_le_bytes());
    zip_bytes.extend_from_slice(&4u32.to_le_bytes());
    zip_bytes.extend_from_slice(&1u16.to_le_bytes());
    zip_bytes.extend_from_slice(&35u16.to_le_bytes());
    zip_bytes.push(b'a');
    zip_bytes.extend_from_slice(&[0u8; 4]);

    // LFH #2 at offset 35..66 (declares extra_len = 4 -> data_start_2 = 70, data_2 = 70..74)
    zip_bytes.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
    zip_bytes.extend_from_slice(&20u16.to_le_bytes());
    zip_bytes.extend_from_slice(&0u16.to_le_bytes());
    zip_bytes.extend_from_slice(&0u16.to_le_bytes());
    zip_bytes.extend_from_slice(&0u16.to_le_bytes());
    zip_bytes.extend_from_slice(&0x21u16.to_le_bytes());
    zip_bytes.extend_from_slice(&crc2.to_le_bytes());
    zip_bytes.extend_from_slice(&4u32.to_le_bytes());
    zip_bytes.extend_from_slice(&4u32.to_le_bytes());
    zip_bytes.extend_from_slice(&1u16.to_le_bytes());
    zip_bytes.extend_from_slice(&4u16.to_le_bytes());
    zip_bytes.push(b'b');

    // Contiguous non-overlapping data payloads: 66..70 and 70..74
    zip_bytes.extend_from_slice(data1);
    zip_bytes.extend_from_slice(data2);

    let cd_offset = zip_bytes.len() as u32;
    for (name, crc, lfh_offset) in [(b'a', crc1, 0u32), (b'b', crc2, 35u32)] {
        zip_bytes.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
        zip_bytes.extend_from_slice(&20u16.to_le_bytes());
        zip_bytes.extend_from_slice(&20u16.to_le_bytes());
        zip_bytes.extend_from_slice(&0u16.to_le_bytes());
        zip_bytes.extend_from_slice(&0u16.to_le_bytes());
        zip_bytes.extend_from_slice(&0u16.to_le_bytes());
        zip_bytes.extend_from_slice(&0x21u16.to_le_bytes());
        zip_bytes.extend_from_slice(&crc.to_le_bytes());
        zip_bytes.extend_from_slice(&4u32.to_le_bytes());
        zip_bytes.extend_from_slice(&4u32.to_le_bytes());
        zip_bytes.extend_from_slice(&1u16.to_le_bytes());
        zip_bytes.extend_from_slice(&[0u8; 12]);
        zip_bytes.extend_from_slice(&lfh_offset.to_le_bytes());
        zip_bytes.push(name);
    }
    let cd_size = (zip_bytes.len() as u32) - cd_offset;
    zip_bytes.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
    zip_bytes.extend_from_slice(&2u16.to_le_bytes());
    zip_bytes.extend_from_slice(&2u16.to_le_bytes());
    zip_bytes.extend_from_slice(&cd_size.to_le_bytes());
    zip_bytes.extend_from_slice(&cd_offset.to_le_bytes());
    zip_bytes.extend_from_slice(&0u16.to_le_bytes());

    let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();
    assert!(!archive.has_overlapping_files().unwrap());
}
