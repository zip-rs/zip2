//! These are tests for the wasm32 architecture
//!
//! ## Tests
//!
//! ```sh
//! rustup target add wasm32-unknown-unknown
//! cargo install wasm-pack
//!
//! # useful commands
//! WASM_BINDGEN_USE_BROWSER=1 wasm-pack test --headless --chrome
//! WASM_BINDGEN_USE_BROWSER=1 wasm-pack test --headless --firefox
//! wasm-pack test --node
//! ```
//!
//! For testing within a browser, you will need the `WASM_BINDGEN_USE_BROWSER=1` env variable
//!

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;

mod aes_encryption;

/// Test AES encryption functionality - run with: wasm-pack test --headless --chrome --features aes-crypto
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
/// Test LZMA decompression functionality in the wasm32 environment.
#[cfg(feature = "lzma")]
#[wasm_bindgen_test]
fn test_decompress_lzma() {
    lzma::decompress_lzma();
}

// The end_to_end tests depend on a time implementation (e.g. the `time` crate or similar)
// that must be compiled with its `wasm-bindgen` feature when targeting `wasm32`.
// Without that feature enabled, running these tests will fail with "time not implemented on this platform".
mod end_to_end;
#[wasm_bindgen_test]
/// Runs the end-to-end integration test suite for wasm32, covering time-dependent behavior.
fn test_end_to_end() {
    end_to_end::end_to_end();
}
