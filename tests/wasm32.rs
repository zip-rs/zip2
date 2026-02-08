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

/// Verifies AES-256 encryption and decryption of an uncompressed file in the `wasm32` environment.
/// The test ensures that data encrypted with AES-256 can be correctly decrypted back to its original
/// contents using the WebAssembly build of the library.
///
/// Run with: `wasm-pack test --headless --chrome --features aes-crypto`
#[cfg(feature = "aes-crypto")]
#[wasm_bindgen_test]
fn test_aes256_encrypted_uncompressed_file() {
    aes_encryption::aes256_encrypted_uncompressed_file();
}

mod deflate64;
/// Verifies Deflate64 decompression functionality in the `wasm32` environment.
/// This test ensures that data compressed using the Deflate64 algorithm can be correctly
/// decompressed using the WebAssembly build of the library.
///
/// Run with: `wasm-pack test --headless --chrome --features deflate64`
#[cfg(feature = "deflate64")]
#[wasm_bindgen_test]
fn test_decompress_deflate64() {
    deflate64::decompress_deflate64();
}

mod xz;
/// Verifies XZ decompression functionality in the `wasm32` environment.
/// This test ensures that data compressed with XZ can be correctly decompressed
/// using the WebAssembly build of the library.
///
/// Run with: `wasm-pack test --headless --chrome --features xz`
#[cfg(feature = "xz")]
#[wasm_bindgen_test]
fn test_decompress_xz() {
    xz::decompress_xz();
}

mod lzma;
/// Verifies LZMA decompression functionality in the `wasm32` environment.
/// The test ensures that data compressed with LZMA can be correctly decompressed
/// using the WebAssembly build of the library.
///
/// Run with: `wasm-pack test --headless --chrome --features lzma`
#[cfg(feature = "lzma")]
#[wasm_bindgen_test]
fn test_decompress_lzma() {
    lzma::decompress_lzma();
}

// The end_to_end tests depend on a time implementation (e.g. the `time` crate or similar)
// that must be compiled with its `wasm-bindgen` feature when targeting `wasm32`.
// Without that feature enabled, running these tests will fail with "time not implemented on this platform".
mod end_to_end;
/// Runs the end-to-end integration test suite for the `wasm32` target, including
/// checks for time-dependent behavior.
///
/// These tests require a time implementation (for example, the `time` crate or a
/// compatible alternative) that is compiled with its `wasm-bindgen` feature when
/// targeting `wasm32`. Without that feature enabled, running this test will fail
/// at runtime with "time not implemented on this platform".
///
/// Run with: `wasm-pack test --headless --chrome` (ensure the appropriate time
/// implementation and its `wasm-bindgen` feature are enabled for `wasm32`).
#[wasm_bindgen_test]
fn test_end_to_end() {
    end_to_end::end_to_end();
}
