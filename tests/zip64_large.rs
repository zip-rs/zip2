// The following is a hexdump of a zip64 file containing the following files:
// zero4400: 4400 MB of zeroes
// zero100: 100 MB of zeroes
// zero4400_2: 4400 MB of zeroes
//
// 00000000  50 4b 03 04 2d 00 00 00  00 00 1b 6e 51 4d 66 82  |PK..-......nQMf.|
// 00000010  13 da ff ff ff ff ff ff  ff ff 08 00 30 00 7a 65  |............0.ze|
// 00000020  72 6f 34 34 30 30 55 54  09 00 03 a5 21 c7 5b db  |ro4400UT....!.[.|
// 00000030  21 c7 5b 75 78 0b 00 01  04 e8 03 00 00 04 e8 03  |!.[ux...........|
// 00000040  00 00 01 00 10 00 00 00  00 13 01 00 00 00 00 00  |................|
// 00000050  00 13 01 00 00 00 00 00  00 00 00 00 00 00 00 00  |................|
// 00000060  00 00 00 00 00 00 00 00  00 00 00 00 00 00 00 00  |................|
// *
// 113000050  00 00 00 00 00 00 50 4b  03 04 0a 00 00 00 00 00  |......PK........|
// 113000060  2b 6e 51 4d 98 23 28 4b  00 00 40 06 00 00 40 06  |+nQM.#(K..@...@.|
// 113000070  07 00 1c 00 7a 65 72 6f  31 30 30 55 54 09 00 03  |....zero100UT...|
// 113000080  c2 21 c7 5b c2 21 c7 5b  75 78 0b 00 01 04 e8 03  |.!.[.!.[ux......|
// 113000090  00 00 04 e8 03 00 00 00  00 00 00 00 00 00 00 00  |................|
// 1130000a0  00 00 00 00 00 00 00 00  00 00 00 00 00 00 00 00  |................|
// *
// 119400090  00 00 00 00 00 00 00 50  4b 03 04 2d 00 00 00 00  |.......PK..-....|
// 1194000a0  00 3b 6e 51 4d 66 82 13  da ff ff ff ff ff ff ff  |.;nQMf..........|
// 1194000b0  ff 0a 00 30 00 7a 65 72  6f 34 34 30 30 5f 32 55  |...0.zero4400_2U|
// 1194000c0  54 09 00 03 e2 21 c7 5b  db 21 c7 5b 75 78 0b 00  |T....!.[.!.[ux..|
// 1194000d0  01 04 e8 03 00 00 04 e8  03 00 00 01 00 10 00 00  |................|
// 1194000e0  00 00 13 01 00 00 00 00  00 00 13 01 00 00 00 00  |................|
// 1194000f0  00 00 00 00 00 00 00 00  00 00 00 00 00 00 00 00  |................|
// *
// 22c4000e0  00 00 00 00 00 00 00 00  00 00 00 00 00 00 00 50  |...............P|
// 22c4000f0  4b 01 02 1e 03 2d 00 00  00 00 00 1b 6e 51 4d 66  |K....-......nQMf|
// 22c400100  82 13 da ff ff ff ff ff  ff ff ff 08 00 2c 00 00  |.............,..|
// 22c400110  00 00 00 00 00 00 00 a4  81 00 00 00 00 7a 65 72  |.............zer|
// 22c400120  6f 34 34 30 30 55 54 05  00 03 a5 21 c7 5b 75 78  |o4400UT....!.[ux|
// 22c400130  0b 00 01 04 e8 03 00 00  04 e8 03 00 00 01 00 10  |................|
// 22c400140  00 00 00 00 13 01 00 00  00 00 00 00 13 01 00 00  |................|
// 22c400150  00 50 4b 01 02 1e 03 0a  00 00 00 00 00 2b 6e 51  |.PK..........+nQ|
// 22c400160  4d 98 23 28 4b 00 00 40  06 00 00 40 06 07 00 24  |M.#(K..@...@...$|
// 22c400170  00 00 00 00 00 00 00 00  00 a4 81 ff ff ff ff 7a  |...............z|
// 22c400180  65 72 6f 31 30 30 55 54  05 00 03 c2 21 c7 5b 75  |ero100UT....!.[u|
// 22c400190  78 0b 00 01 04 e8 03 00  00 04 e8 03 00 00 01 00  |x...............|
// 22c4001a0  08 00 56 00 00 13 01 00  00 00 50 4b 01 02 1e 03  |..V.......PK....|
// 22c4001b0  2d 00 00 00 00 00 3b 6e  51 4d 66 82 13 da ff ff  |-.....;nQMf.....|
// 22c4001c0  ff ff ff ff ff ff 0a 00  34 00 00 00 00 00 00 00  |........4.......|
// 22c4001d0  00 00 a4 81 ff ff ff ff  7a 65 72 6f 34 34 30 30  |........zero4400|
// 22c4001e0  5f 32 55 54 05 00 03 e2  21 c7 5b 75 78 0b 00 01  |_2UT....!.[ux...|
// 22c4001f0  04 e8 03 00 00 04 e8 03  00 00 01 00 18 00 00 00  |................|
// 22c400200  00 13 01 00 00 00 00 00  00 13 01 00 00 00 97 00  |................|
// 22c400210  40 19 01 00 00 00 50 4b  06 06 2c 00 00 00 00 00  |@.....PK..,.....|
// 22c400220  00 00 1e 03 2d 00 00 00  00 00 00 00 00 00 03 00  |....-...........|
// 22c400230  00 00 00 00 00 00 03 00  00 00 00 00 00 00 27 01  |..............'.|
// 22c400240  00 00 00 00 00 00 ef 00  40 2c 02 00 00 00 50 4b  |........@,....PK|
// 22c400250  06 07 00 00 00 00 16 02  40 2c 02 00 00 00 01 00  |........@,......|
// 22c400260  00 00 50 4b 05 06 00 00  00 00 03 00 03 00 27 01  |..PK..........'.|
// 22c400270  00 00 ff ff ff ff 00 00                           |........|
// 22c400278
use std::{
    fs::File,
    io::{self, Cursor, Read, Seek, SeekFrom},
    path::Path,
};

use zip::write::SimpleFileOptions;

const BLOCK1_LENGTH: u64 = 0x60;
const BLOCK1: [u8; BLOCK1_LENGTH as usize] = [
    0x50, 0x4b, 0x03, 0x04, 0x2d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1b, 0x6e, 0x51, 0x4d, 0x66, 0x82,
    0x13, 0xda, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x08, 0x00, 0x30, 0x00, 0x7a, 0x65,
    0x72, 0x6f, 0x34, 0x34, 0x30, 0x30, 0x55, 0x54, 0x09, 0x00, 0x03, 0xa5, 0x21, 0xc7, 0x5b, 0xdb,
    0x21, 0xc7, 0x5b, 0x75, 0x78, 0x0b, 0x00, 0x01, 0x04, 0xe8, 0x03, 0x00, 0x00, 0x04, 0xe8, 0x03,
    0x00, 0x00, 0x01, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x13, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x13, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

const BLOCK2_LENGTH: u64 = 0x50;
const BLOCK2: [u8; BLOCK2_LENGTH as usize] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x50, 0x4b, 0x03, 0x04, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x2b, 0x6e, 0x51, 0x4d, 0x98, 0x23, 0x28, 0x4b, 0x00, 0x00, 0x40, 0x06, 0x00, 0x00, 0x40, 0x06,
    0x07, 0x00, 0x1c, 0x00, 0x7a, 0x65, 0x72, 0x6f, 0x31, 0x30, 0x30, 0x55, 0x54, 0x09, 0x00, 0x03,
    0xc2, 0x21, 0xc7, 0x5b, 0xc2, 0x21, 0xc7, 0x5b, 0x75, 0x78, 0x0b, 0x00, 0x01, 0x04, 0xe8, 0x03,
    0x00, 0x00, 0x04, 0xe8, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

const BLOCK3_LENGTH: u64 = 0x60;
const BLOCK3: [u8; BLOCK3_LENGTH as usize] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x50, 0x4b, 0x03, 0x04, 0x2d, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x3b, 0x6e, 0x51, 0x4d, 0x66, 0x82, 0x13, 0xda, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0x0a, 0x00, 0x30, 0x00, 0x7a, 0x65, 0x72, 0x6f, 0x34, 0x34, 0x30, 0x30, 0x5f, 0x32, 0x55,
    0x54, 0x09, 0x00, 0x03, 0xe2, 0x21, 0xc7, 0x5b, 0xdb, 0x21, 0xc7, 0x5b, 0x75, 0x78, 0x0b, 0x00,
    0x01, 0x04, 0xe8, 0x03, 0x00, 0x00, 0x04, 0xe8, 0x03, 0x00, 0x00, 0x01, 0x00, 0x10, 0x00, 0x00,
    0x00, 0x00, 0x13, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x13, 0x01, 0x00, 0x00, 0x00, 0x00,
];

const BLOCK4_LENGTH: u64 = 0x198;
const BLOCK4: [u8; BLOCK4_LENGTH as usize] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x50,
    0x4b, 0x01, 0x02, 0x1e, 0x03, 0x2d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1b, 0x6e, 0x51, 0x4d, 0x66,
    0x82, 0x13, 0xda, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x08, 0x00, 0x2c, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xa4, 0x81, 0x00, 0x00, 0x00, 0x00, 0x7a, 0x65, 0x72,
    0x6f, 0x34, 0x34, 0x30, 0x30, 0x55, 0x54, 0x05, 0x00, 0x03, 0xa5, 0x21, 0xc7, 0x5b, 0x75, 0x78,
    0x0b, 0x00, 0x01, 0x04, 0xe8, 0x03, 0x00, 0x00, 0x04, 0xe8, 0x03, 0x00, 0x00, 0x01, 0x00, 0x10,
    0x00, 0x00, 0x00, 0x00, 0x13, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x13, 0x01, 0x00, 0x00,
    0x00, 0x50, 0x4b, 0x01, 0x02, 0x1e, 0x03, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2b, 0x6e, 0x51,
    0x4d, 0x98, 0x23, 0x28, 0x4b, 0x00, 0x00, 0x40, 0x06, 0x00, 0x00, 0x40, 0x06, 0x07, 0x00, 0x24,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xa4, 0x81, 0xff, 0xff, 0xff, 0xff, 0x7a,
    0x65, 0x72, 0x6f, 0x31, 0x30, 0x30, 0x55, 0x54, 0x05, 0x00, 0x03, 0xc2, 0x21, 0xc7, 0x5b, 0x75,
    0x78, 0x0b, 0x00, 0x01, 0x04, 0xe8, 0x03, 0x00, 0x00, 0x04, 0xe8, 0x03, 0x00, 0x00, 0x01, 0x00,
    0x08, 0x00, 0x56, 0x00, 0x00, 0x13, 0x01, 0x00, 0x00, 0x00, 0x50, 0x4b, 0x01, 0x02, 0x1e, 0x03,
    0x2d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3b, 0x6e, 0x51, 0x4d, 0x66, 0x82, 0x13, 0xda, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x0a, 0x00, 0x34, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0xa4, 0x81, 0xff, 0xff, 0xff, 0xff, 0x7a, 0x65, 0x72, 0x6f, 0x34, 0x34, 0x30, 0x30,
    0x5f, 0x32, 0x55, 0x54, 0x05, 0x00, 0x03, 0xe2, 0x21, 0xc7, 0x5b, 0x75, 0x78, 0x0b, 0x00, 0x01,
    0x04, 0xe8, 0x03, 0x00, 0x00, 0x04, 0xe8, 0x03, 0x00, 0x00, 0x01, 0x00, 0x18, 0x00, 0x00, 0x00,
    0x00, 0x13, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x13, 0x01, 0x00, 0x00, 0x00, 0x97, 0x00,
    0x40, 0x19, 0x01, 0x00, 0x00, 0x00, 0x50, 0x4b, 0x06, 0x06, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x1e, 0x03, 0x2d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x27, 0x01,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xef, 0x00, 0x40, 0x2c, 0x02, 0x00, 0x00, 0x00, 0x50, 0x4b,
    0x06, 0x07, 0x00, 0x00, 0x00, 0x00, 0x16, 0x02, 0x40, 0x2c, 0x02, 0x00, 0x00, 0x00, 0x01, 0x00,
    0x00, 0x00, 0x50, 0x4b, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x03, 0x00, 0x27, 0x01,
    0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00,
];

const BLOCK1_START: u64 = 0x000000000;
const BLOCK2_START: u64 = 0x113000050;
const BLOCK3_START: u64 = 0x119400090;
const BLOCK4_START: u64 = 0x22c4000e0;

const BLOCK1_END: u64 = BLOCK1_START + BLOCK1_LENGTH - 1;
const BLOCK2_END: u64 = BLOCK2_START + BLOCK2_LENGTH - 1;
const BLOCK3_END: u64 = BLOCK3_START + BLOCK3_LENGTH - 1;
const BLOCK4_END: u64 = BLOCK4_START + BLOCK4_LENGTH - 1;

const TOTAL_LENGTH: u64 = BLOCK4_START + BLOCK4_LENGTH;

struct Zip64File {
    pointer: u64,
}

impl Zip64File {
    fn new() -> Self {
        Zip64File { pointer: 0 }
    }
}

impl Seek for Zip64File {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(offset) => {
                self.pointer = offset;
            }
            SeekFrom::End(offset) => {
                if offset > 0 || offset < -(TOTAL_LENGTH as i64) {
                    return Err(io::Error::other("Invalid seek offset"));
                }
                self.pointer = (TOTAL_LENGTH as i64 + offset) as u64;
            }
            SeekFrom::Current(offset) => {
                let seekpos = self.pointer as i64 + offset;
                if seekpos < 0 || seekpos as u64 > TOTAL_LENGTH {
                    return Err(io::Error::other("Invalid seek offset"));
                }
                self.pointer = seekpos as u64;
            }
        }
        Ok(self.pointer)
    }
}

impl Read for Zip64File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pointer >= TOTAL_LENGTH {
            return Ok(0);
        }
        match self.pointer {
            BLOCK1_START..=BLOCK1_END => {
                buf[0] = BLOCK1[(self.pointer - BLOCK1_START) as usize];
            }
            BLOCK2_START..=BLOCK2_END => {
                buf[0] = BLOCK2[(self.pointer - BLOCK2_START) as usize];
            }
            BLOCK3_START..=BLOCK3_END => {
                buf[0] = BLOCK3[(self.pointer - BLOCK3_START) as usize];
            }
            BLOCK4_START..=BLOCK4_END => {
                buf[0] = BLOCK4[(self.pointer - BLOCK4_START) as usize];
            }
            _ => {
                buf[0] = 0;
            }
        }
        self.pointer += 1;
        Ok(1)
    }
}

#[test]
fn zip64_large() {
    let zipfile = Zip64File::new();
    let mut archive = zip::ZipArchive::new(zipfile).unwrap();
    let mut buf = [0u8; 32];

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).unwrap();
        let outpath = file.enclosed_name().unwrap();
        println!(
            "Entry {} has name \"{}\" ({} bytes)",
            i,
            outpath.display(),
            file.size()
        );

        match file.read_exact(&mut buf) {
            Ok(()) => println!("The first {} bytes are: {:?}", buf.len(), buf),
            Err(e) => println!("Could not read the file: {e:?}"),
        };
    }
}

/// We cannot run this test because on wasm32
/// the literal `5368709808` does not fit into the type `usize` whose range is `0..=4294967295`
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_zip64_check_extra_field() {
    let path = Path::new("bigfile.bin");
    let bigfile = File::create(path).expect("Failed to create a big file");

    let bigfile_size: u32 = 1024 * 1024 * 1024;
    // 1024 MiB = 1024 * 1024 * 1024 bytes
    bigfile
        .set_len(bigfile_size as u64)
        .expect("Failed to set file length of the big file");

    let mut archive_buffer = Vec::new();
    let res = zip64_check_extra_field(path, &mut archive_buffer);
    std::fs::remove_file(path).expect("Failed to remove the big file");

    assert_eq!(res.unwrap(), ());
    assert_eq!(archive_buffer.len(), 5368709808);

    // uncomment for debug
    // use std::io::Write;
    // let mut file = File::create("tests/data/test_zip64_check_extra_field.zip").unwrap();
    // file.write_all(&archive_buffer).unwrap();

    let mut read_archive =
        zip::ZipArchive::new(Cursor::new(&archive_buffer)).expect("Failed to read the archive");

    assert_eq!(read_archive.len(), 4 + 1 + 1); // the archive should contain 4 files + 1 directory + 1 file in the directory
    {
        let dir = read_archive.by_name("dir/").unwrap();
        assert_eq!(dir.compressed_size(), 0);
        assert_eq!(dir.size(), 0);
        let header_start = 4294967452;
        assert_eq!(dir.header_start(), header_start);
        assert_eq!(dir.central_header_start(), 5368709575);
        let central_header_start = dir.central_header_start() as usize;
        let central_header_end = central_header_start + 62;
        // take a bunch of bytes from the central file header of the directory entry, which should contain the zip64 extra field
        let range = central_header_start..central_header_end;
        let central_header = archive_buffer.get(range).unwrap();
        assert_eq!(central_header[0..4], [0x50, 0x4b, 0x01, 0x02]); // central file header signature
        // assert_eq!(central_header[4..6], [0x14, 0x03]); // version made by
        assert_eq!(central_header[6..8], [0x14, 0x00]); // version needed to extract
        assert_eq!(central_header[8..10], [0x00, 0x00]); // general purpose bit flag
        assert_eq!(central_header[10..12], [0x00, 0x00]); // compression method
        // assert_eq!(raw_access[12..14], [0x00, 0x00]); // last mod file time
        // assert_eq!(raw_access[14..16], [0x00, 0x00]); // last mod file date
        assert_eq!(central_header[16..20], [0x00, 0x00, 0x00, 0x00]); // crc-32
        assert_eq!(central_header[20..24], [0x00, 0x00, 0x00, 0x00]); // compressed size - IMPORTANT
        assert_eq!(central_header[24..28], [0x00, 0x00, 0x00, 0x00]); // uncompressed size - IMPORTANT
        assert_eq!(central_header[28..30], [0x04, 0x00]); // file name length
        // IMPORTANT
        assert_eq!(central_header[30..32], [0x0c, 0x00]); // extra field length
        assert_eq!(central_header[32..34], [0x00, 0x00]); // file comment length
        assert_eq!(central_header[34..36], [0x00, 0x00]); // disk number start
        assert_eq!(central_header[36..38], [0x00, 0x00]); // internal file attributes
        // assert_eq!(raw_access[38..42], [0x00, 0x00, 0x00, 0x00]); // external file attributes
        // IMPORTANT
        assert_eq!(central_header[42..46], [0xFF, 0xFF, 0xFF, 0xFF]); // relative offset of local header
        assert_eq!(central_header[46..50], *b"dir/"); // file name
        assert_eq!(central_header[50..52], [0x01, 0x00]); // zip64 extra field header id
        assert_eq!(central_header[52..54], [0x08, 0x00]); // zip64 extra field data size (should be 0 for a directory entry, since
        // IMPORTANT
        assert_eq!(
            central_header[54..],
            [0x9c, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00]
        ); // zip64 extra field
        assert_eq!(central_header[54..], dir.header_start().to_le_bytes());

        // now we check the local header
        let local_block_start = dir.header_start() as usize;
        let local_block_end = (dir.header_start() + 33) as usize;
        let range_local_block = local_block_start..=local_block_end;
        let local_block = archive_buffer.get(range_local_block).unwrap();
        eprintln!("local_block = {:x?}", local_block);
        assert_eq!(local_block[0..4], [0x50, 0x4b, 0x03, 0x04]); // local header signature
        assert_eq!(local_block[4..6], [0x14, 0x00]); // version
        assert_eq!(local_block[6..8], [0x00, 0x00]); // flags
        assert_eq!(local_block[8..10], [0x00, 0x00]); // compression
        // assert_eq!(local_block[10..12], [0x00, 0x00]); // time
        // assert_eq!(local_block[12..14], [0x00, 0x00]); // date
        assert_eq!(local_block[14..18], [0x00, 0x00, 0x00, 0x00]); // crc 32
        assert_eq!(local_block[18..22], [0x00, 0x00, 0x00, 0x00]); // compressed size
        assert_eq!(local_block[22..26], [0x00, 0x00, 0x00, 0x00]); // uncompressed size
        assert_eq!(local_block[26..28], [0x04, 0x00]); // file name length
        assert_eq!(local_block[28..30], [0x00, 0x00]); // extra field length
        assert_eq!(local_block[30..], *b"dir/"); // file name
        // there is not zip64 extra field in the local header
    }
    {
        let bigfile_archive = read_archive.by_name("dir/bigfile.bin").unwrap();
        assert_eq!(bigfile_archive.compressed_size(), 1024 * 1024 * 1024);
        assert_eq!(bigfile_archive.size(), 1024 * 1024 * 1024);
        let header_start = 4294967486;
        assert_eq!(bigfile_archive.header_start(), header_start);
        assert_eq!(bigfile_archive.central_header_start(), 5368709637);

        let central_header_start = bigfile_archive.central_header_start() as usize;
        // take a bunch of bytes from the central file header of the file entry, which should contain the zip64 extra field
        let central_header_end = central_header_start + 73;
        let range = central_header_start..central_header_end;
        let central_header = archive_buffer.get(range).unwrap();
        assert_eq!(central_header[0..4], [0x50, 0x4b, 0x01, 0x02]); // central file header signature
        // assert_eq!(central_header[4..6], [0x0A, 0x03]); // version made by
        assert_eq!(central_header[6..8], [0x0A, 0x00]); // version needed to extract
        assert_eq!(central_header[8..10], [0x00, 0x00]); // general purpose bit flag
        assert_eq!(central_header[10..12], [0x00, 0x00]); // compression method
        // assert_eq!(raw_access[12..14], [0x00, 0x00]); // last mod file time
        // assert_eq!(raw_access[14..16], [0x00, 0x00]); // last mod file date
        assert_eq!(central_header[16..20], [0xB0, 0xC2, 0x64, 0x5B]); // crc-32
        assert_eq!(central_header[20..24], bigfile_size.to_le_bytes()); // compressed size - IMPORTANT
        assert_eq!(central_header[24..28], bigfile_size.to_le_bytes()); // uncompressed size - IMPORTANT
        assert_eq!(central_header[28..30], [0x0f, 0x00]); // file name length
        // IMPORTANT
        assert_eq!(central_header[30..32], [0x0c, 0x00]); // extra field length
        assert_eq!(central_header[32..34], [0x00, 0x00]); // file comment length
        assert_eq!(central_header[34..36], [0x00, 0x00]); // disk number start
        assert_eq!(central_header[36..38], [0x00, 0x00]); // internal file attributes
        // assert_eq!(raw_access[38..42], [0x00, 0x00, 0x00, 0x00]); // external file attributes
        // IMPORTANT
        assert_eq!(central_header[42..46], [0xFF, 0xFF, 0xFF, 0xFF]); // relative offset of local header
        assert_eq!(central_header[46..61], *b"dir/bigfile.bin"); // file name
        assert_eq!(central_header[61..63], [0x01, 0x00]); // zip64 extra field header id
        assert_eq!(central_header[63..65], [0x08, 0x00]); // zip64 extra field data size
        assert_eq!(
            central_header[65..],
            [0xbe, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00]
        ); // zip64 extra field data Relative Header Offset
        assert_eq!(central_header[65..], header_start.to_le_bytes()); // the offset in the zip64 extra field should match the header start of the file data

        // now we check the local header
        let local_header_start = bigfile_archive.header_start() as usize;
        let local_header_end = local_header_start + 45;
        let range = local_header_start..local_header_end;
        let local_header = archive_buffer.get(range).unwrap();
        eprintln!("RAW ACCESS: {:x?}", local_header);
        assert_eq!(local_header[0..4], [0x50, 0x4b, 0x03, 0x04]); // local file header signature
        assert_eq!(local_header[4..6], [0x0A, 0x00]); // version needed to extract
        assert_eq!(local_header[6..8], [0x00, 0x00]); // general purpose bit flag
        assert_eq!(local_header[8..10], [0x00, 0x00]); // compression method
        // assert_eq!(raw_access[10..12], [0x00, 0x00]); // last mod file time
        // assert_eq!(raw_access[12..14], [0x00, 0x00]); // last mod file date
        assert_eq!(local_header[14..18], [176, 194, 100, 91]); // crc-32
        assert_eq!(local_header[18..22], bigfile_size.to_le_bytes()); // compressed size
        assert_eq!(local_header[22..26], bigfile_size.to_le_bytes()); // uncompressed size
        assert_eq!(local_header[26..28], [0x0f, 0x00]); // file name length
        // IMPORTANT
        assert_eq!(local_header[28..30], [0x00, 0x00]); // extra field length
        assert_eq!(local_header[30..], *b"dir/bigfile.bin"); // file name
    }
}

fn zip64_check_extra_field(
    path: &Path,
    archive_buffer: &mut Vec<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut bigfile = File::open(path)?;

    let mut archive = zip::ZipWriter::new(Cursor::new(archive_buffer));
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o755);

    // add 4GiB of file data to the archive, which should trigger the zip64 extra field
    for i in 0..4 {
        bigfile.seek(SeekFrom::Start(0))?;
        let filename = format!("file{}.bin", i + 1);
        archive.start_file(filename, options)?;
        std::io::copy(&mut bigfile, &mut archive)?;
    }
    // now add a directory entry, which SHOULD trigger the zip64 extra field for the central directory header
    archive.add_directory("dir/", options)?;
    archive.start_file("dir/bigfile.bin", options)?;
    bigfile.seek(SeekFrom::Start(0))?;
    std::io::copy(&mut bigfile, &mut archive)?;
    archive.finish()?;
    Ok(())
}
