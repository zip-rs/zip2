mod huffman;
mod lz77;
pub mod shrink;
pub use shrink::*;
pub mod reduce;
pub use reduce::*;
pub mod implode;
pub use implode::*;
/// Reverse the n least significant bits of x.
/// The (16 - n) most significant bits of the result will be zero.
pub fn reverse_lsb(x: u16, n: usize) -> u16 {
    debug_assert!(n > 0);
    debug_assert!(n <= 16);
    x.reverse_bits() >> (16 - n)
}
/// Get the n least significant bits of x.
pub fn lsb(x: u64, n: u8) -> u64 {
    debug_assert!(n <= 63);
    x & ((1u64 << (n as u32)) - 1)
}
