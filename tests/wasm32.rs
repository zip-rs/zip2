//! These are test for the wasm32 arch
//!
//! ## Tests
//!
//! ```sh
//! rustup target add wasm32-unknown-unknown
//! cargo install wasm-pack
//!
//! # useful commands
//! wasm-pack test --headless --chrome
//! wasm-pack test --firefox
//! wasm-pack test --node
//! ```
//!
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::*;
wasm_bindgen_test_configure!(run_in_browser);

use std::io::{self, Read};
use zip::ZipArchive;

const SECRET_CONTENT: &str = "Lorem ipsum dolor sit amet";

const PASSWORD: &[u8] = b"helloworld";

/// wasm-pack test --headless --chrome --features aes-crypto
#[wasm_bindgen_test]
#[cfg(feature = "aes-crypto")]
fn aes256_encrypted_uncompressed_file() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("data/aes_archive.zip"));
    let mut archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    let mut file = archive
        .by_name_decrypt("secret_data_256_uncompressed", PASSWORD)
        .expect("couldn't find file in archive");
    assert_eq!("secret_data_256_uncompressed", file.name());

    let mut content = String::new();
    file.read_to_string(&mut content)
        .expect("couldn't read encrypted file");
    assert_eq!(SECRET_CONTENT, content);
}
