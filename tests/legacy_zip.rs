#![cfg(feature = "legacy-zip")]

use std::io::{self, Read};
use zip::ZipArchive;

#[test]
fn decompress_shrink() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/legacy/shrink.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    let mut file = archive
        .by_name("FIRST.TXT")
        .expect("couldn't find file in archive");
    let file_name = file.name().unwrap();
    assert_eq!("FIRST.TXT", file_name);

    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .expect("couldn't read encrypted and compressed file");
    assert_eq!(include_bytes!("data/folder/first.txt"), &content[..]);
}

#[test]
fn decompress_reduce() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/legacy/reduce.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    let mut file = archive
        .by_name("first.txt")
        .expect("couldn't find file in archive");
    let file_name = file.name().unwrap();
    assert_eq!("first.txt", file_name);

    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .expect("couldn't read encrypted and compressed file");
    assert_eq!(include_bytes!("data/folder/first.txt"), &content[..]);
}

#[test]
fn decompress_implode() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/legacy/implode.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    let mut file = archive
        .by_name("first.txt")
        .expect("couldn't find file in archive");
    let file_name = file.name().unwrap();
    assert_eq!("first.txt", file_name);

    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .expect("couldn't read encrypted and compressed file");
    assert_eq!(include_bytes!("data/folder/first.txt"), &content[..]);
}

const ZIP64_SENTINEL: u32 = 0xFFFF_FFFF;

fn u16le(v: u16) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

fn u32le(v: u32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

fn u64le(v: u64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

/// A ~150 byte archive with a single entry that uses `method` and *declares*
/// `declared_size` uncompressed bytes while storing ten.
///
/// Nothing here is ever decoded successfully: the point of the fixture is the
/// declared size, which is what a hostile archive gets to choose.
fn hostile_archive(method: u16, declared_size: u64) -> Vec<u8> {
    let name: &[u8] = b"a";
    let payload = [0u8; 10];
    let mut out = Vec::new();

    // ---- local file header ----
    out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
    out.extend_from_slice(&u16le(45)); // version needed (zip64)
    out.extend_from_slice(&u16le(0)); // flags
    out.extend_from_slice(&u16le(method));
    out.extend_from_slice(&u16le(0)); // mod time
    out.extend_from_slice(&u16le(0x21)); // mod date
    out.extend_from_slice(&u32le(0)); // crc32
    out.extend_from_slice(&u32le(ZIP64_SENTINEL)); // compressed size -> zip64
    out.extend_from_slice(&u32le(ZIP64_SENTINEL)); // uncompressed size -> zip64
    out.extend_from_slice(&u16le(name.len() as u16));
    out.extend_from_slice(&u16le(20)); // zip64 extra field
    out.extend_from_slice(name);
    out.extend_from_slice(&u16le(0x0001));
    out.extend_from_slice(&u16le(16));
    out.extend_from_slice(&u64le(declared_size));
    out.extend_from_slice(&u64le(payload.len() as u64));
    out.extend_from_slice(&payload);

    let cd_offset = out.len() as u32;

    // ---- central directory header ----
    out.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
    out.extend_from_slice(&u16le(0x031E)); // version made by: Unix
    out.extend_from_slice(&u16le(45));
    out.extend_from_slice(&u16le(0));
    out.extend_from_slice(&u16le(method));
    out.extend_from_slice(&u16le(0));
    out.extend_from_slice(&u16le(0x21));
    out.extend_from_slice(&u32le(0));
    out.extend_from_slice(&u32le(ZIP64_SENTINEL));
    out.extend_from_slice(&u32le(ZIP64_SENTINEL));
    out.extend_from_slice(&u16le(name.len() as u16));
    out.extend_from_slice(&u16le(20));
    out.extend_from_slice(&u16le(0)); // comment len
    out.extend_from_slice(&u16le(0)); // disk number start
    out.extend_from_slice(&u16le(0)); // internal attrs
    out.extend_from_slice(&u32le(0)); // external attrs
    out.extend_from_slice(&u32le(0)); // local header offset
    out.extend_from_slice(name);
    out.extend_from_slice(&u16le(0x0001));
    out.extend_from_slice(&u16le(16));
    out.extend_from_slice(&u64le(declared_size));
    out.extend_from_slice(&u64le(payload.len() as u64));

    let cd_size = out.len() as u32 - cd_offset;

    // ---- end of central directory ----
    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
    out.extend_from_slice(&u16le(0));
    out.extend_from_slice(&u16le(0));
    out.extend_from_slice(&u16le(1));
    out.extend_from_slice(&u16le(1));
    out.extend_from_slice(&u32le(cd_size));
    out.extend_from_slice(&u32le(cd_offset));
    out.extend_from_slice(&u16le(0));

    out
}

/// The declared uncompressed size is read straight out of the header and is
/// therefore attacker-controlled. Reserving it up front let a ~150 byte
/// archive ask for an arbitrarily large allocation: 1 TiB aborted the process
/// on a failed `malloc`, and `u64::MAX` panicked with `capacity overflow`.
/// Neither is catchable by the caller, and both contradict the crate's promise
/// of not panicking on malformed input.
///
/// With the pre-allocation gone the same archives must fail like any other
/// malformed input, from inside the decompressor.
#[test]
fn legacy_decompressors_do_not_pre_allocate_the_declared_size() {
    // 1 = Shrink, 4 = Reduce (factor 4), 6 = Implode.
    for method in [1u16, 4, 6] {
        for declared in [1u64 << 40, 1u64 << 62, u64::MAX] {
            let archive = hostile_archive(method, declared);
            let mut zip = ZipArchive::new(io::Cursor::new(archive)).expect("archive must parse");

            let mut file = zip.by_index(0).expect("entry must be readable");
            let mut content = Vec::new();
            let result = file.read_to_end(&mut content);

            assert!(
                result.is_err(),
                "method {} declared {} -- must surface as an error, not an abort",
                method,
                declared
            );
        }
    }
}
