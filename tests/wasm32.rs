//! These are tests for the wasm32 architecture
//!
//! ## Tests
//!
//! ```sh
//! rustup target add wasm32-unknown-unknown
//! cargo install wasm-pack
//!
//! # useful commands
//! wasm-pack test --headless --chrome
//! wasm-pack test --headless --firefox
//! wasm-pack test --node
//! ```
//!

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;

mod aes_encryption;

/// wasm-pack test --headless --chrome --features aes-crypto
#[cfg(feature = "aes-crypto")]
#[wasm_bindgen_test]
fn test_aes256_encrypted_uncompressed_file() {
    aes_encryption::aes256_encrypted_uncompressed_file();
}

mod deflate64;
#[cfg(feature = "deflate64")]
#[wasm_bindgen_test]
fn test_decompress_deflate64() {
    deflate64::decompress_deflate64();
}

mod xz;
#[cfg(feature = "xz")]
#[wasm_bindgen_test]
fn test_decompress_xz() {
    xz::decompress_xz();
}

mod lzma;
#[cfg(feature = "lzma")]
#[wasm_bindgen_test]
fn test_decompress_lzma() {
    lzma::decompress_lzma();
}

// time needs the features wasm-bindgen
// or you get the error
// time not implemented on this platform
mod end_to_end;
#[wasm_bindgen_test]
fn test_end_to_end() {
    end_to_end::end_to_end();
}
