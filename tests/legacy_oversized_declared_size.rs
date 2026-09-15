#![cfg(feature = "legacy-zip")]

use std::io::{Cursor, Read};
use zip::ZipArchive;

fn build(method: u16, declared_size: u64, payload: &[u8]) -> Vec<u8> {
    const SENTINEL: u32 = 0xFFFF_FFFF;
    let name = b"a";
    let mut o = Vec::new();
    o.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
    o.extend_from_slice(&45u16.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&method.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&0x21u16.to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&SENTINEL.to_le_bytes());
    o.extend_from_slice(&SENTINEL.to_le_bytes());
    o.extend_from_slice(&(name.len() as u16).to_le_bytes());
    o.extend_from_slice(&20u16.to_le_bytes());
    o.extend_from_slice(name);
    o.extend_from_slice(&0x0001u16.to_le_bytes());
    o.extend_from_slice(&16u16.to_le_bytes());
    o.extend_from_slice(&declared_size.to_le_bytes());
    o.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    o.extend_from_slice(payload);

    let cd_offset = o.len() as u32;
    o.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
    o.extend_from_slice(&0x031Eu16.to_le_bytes());
    o.extend_from_slice(&45u16.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&method.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&0x21u16.to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(&SENTINEL.to_le_bytes());
    o.extend_from_slice(&SENTINEL.to_le_bytes());
    o.extend_from_slice(&(name.len() as u16).to_le_bytes());
    o.extend_from_slice(&20u16.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&(0o644u32 << 16).to_le_bytes());
    o.extend_from_slice(&0u32.to_le_bytes());
    o.extend_from_slice(name);
    o.extend_from_slice(&0x0001u16.to_le_bytes());
    o.extend_from_slice(&16u16.to_le_bytes());
    o.extend_from_slice(&declared_size.to_le_bytes());
    o.extend_from_slice(&(payload.len() as u64).to_le_bytes());

    let cd_size = o.len() as u32 - cd_offset;
    o.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&cd_size.to_le_bytes());
    o.extend_from_slice(&cd_offset.to_le_bytes());
    o.extend_from_slice(&0u16.to_le_bytes());
    o
}

fn read_entry(method: u16, declared: u64) {
    eprintln!("PROBE method={method} declared={declared:#x}");
    let data = build(method, declared, b"\x00\x01\x02\x03");
    let mut ar = ZipArchive::new(Cursor::new(data)).expect("open");
    let mut f = ar.by_index(0).expect("by_index");
    let mut out = Vec::new();
    let r = f.read_to_end(&mut out);
    eprintln!(
        "  -> result = {:?}",
        r.as_ref().map(|n| *n).map_err(|e| e.to_string())
    );
}

#[test]
fn shrink_ignores_oversized_declared_size() {
    read_entry(1, u64::MAX);
}
#[test]
fn implode_ignores_oversized_declared_size() {
    read_entry(6, u64::MAX);
}
#[test]
fn reduce_ignores_oversized_declared_size() {
    read_entry(2, u64::MAX);
}
#[test]
fn stored_ignores_oversized_declared_size() {
    read_entry(0, u64::MAX);
}
