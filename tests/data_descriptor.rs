//! Tests with data descriptors

// https://github.com/zip-rs/zip2/issues/971
#[test]
fn directory_should_not_have_a_data_descriptor() {
    use std::io::{Cursor, Write};
    use zip::{HasZipMetadata, ZipArchive, ZipWriter, write::SimpleFileOptions};
    const LOCAL_FILE_HEADER_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
    let mut writer = ZipWriter::new_stream(Vec::new());
    writer
        .add_directory("mydir", SimpleFileOptions::default())
        .unwrap();
    writer
        .start_file("mydir/file.txt", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"hello").unwrap();
    let bytes = writer.finish().unwrap().into_inner();

    let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let directory = archive.by_name("mydir/").unwrap();
    let metadata = directory.get_metadata();
    let data_start = directory.data_start().unwrap() as usize;

    println!(
        "dir: using_data_descriptor={}",
        metadata.flags.is_using_data_descriptor()
    );
    let is_data_descriptor = metadata.flags.is_using_data_descriptor();
    assert!(!is_data_descriptor);

    let next_bytes: [u8; 4] = bytes[data_start..data_start + 4].try_into().unwrap();
    println!("bytes right after dir's data: {next_bytes:02x?}");

    assert_eq!(next_bytes, LOCAL_FILE_HEADER_SIGNATURE);
}

#[test]
fn read_data_descriptor() {
    use std::io::{Cursor, Write};
    use zip::CompressionMethod;
    use zip::unstable::format::magic::Magic;
    use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

    let mut writer = ZipWriter::new_stream(Vec::new());
    writer
        .start_file(
            "file.txt",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(b"hello").unwrap();
    let bytes = writer.finish().unwrap().into_inner();

    let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let file = archive.by_name("file.txt").unwrap();
    let data_start = file.data_start().unwrap() as usize;

    // Also handle big data descriptor
    let desc_start = data_start + file.compressed_size() as usize;
    let desc_end = desc_start + 4 + 4 + 4 + 4;
    let data_descriptor = &bytes[desc_start..desc_end];
    let magic = Magic::DATA_DESCRIPTOR_SIGNATURE.to_le_bytes();
    let crc32 = [134, 166, 16, 54];
    assert_eq!(
        data_descriptor,
        [
            magic[0], magic[1], magic[2], magic[3], // magic
            crc32[0], crc32[1], crc32[2], crc32[3], // crc32
            5, 0, 0, 0, // compressed size
            5, 0, 0, 0 // uncompressed size
        ]
    );
}

#[test]
fn read_data_descriptor_stream() {
    use std::io::{Cursor, Write};
    use zip::CompressionMethod;
    use zip::unstable::format::data_descriptor::{ZipDataDescriptor, ZipDataDescriptorBlock};
    use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

    let mut writer = ZipWriter::new_stream(Vec::new());
    writer
        .start_file(
            "file.txt",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(b"hello").unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let stream = Cursor::new(bytes); // stream is Read + Seek

    let mut archive = ZipArchive::new(stream).unwrap();

    let index = archive.index_for_name("file.txt").unwrap();
    let data = archive.by_index_with_data_descriptor(index).unwrap();
    let data_descriptor = data.data_descriptor().unwrap();
    assert_eq!(
        data_descriptor,
        ZipDataDescriptor::ZipDataDescriptorBlock(ZipDataDescriptorBlock {
            crc32: u32::from_le_bytes([134, 166, 16, 54]),
            compressed_size: 5,
            uncompressed_size: 5,
        })
    );
}

#[test]
fn read_data_descriptor_not_present() {
    use std::io::{Cursor, Write};
    use zip::CompressionMethod;
    use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "file.txt",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(b"hello").unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let stream = Cursor::new(bytes); // stream is Read + Seek

    let mut archive = ZipArchive::new(stream).unwrap();

    let index = archive.index_for_name("file.txt").unwrap();
    let data = archive.by_index_with_data_descriptor(index).unwrap();
    let data_descriptor = data.data_descriptor();
    // this is not a stream, no data_descriptor
    assert!(data_descriptor.is_none());
}
