// Some zip files can contain garbage after the comment. For example, python zipfile generates
// it when opening a zip in 'a' mode:
//
// >>> from zipfile import ZipFile
// >>> with ZipFile('comment_garbage.zip', 'a') as z:
// ...     z.comment = b'long comment bla bla bla'
// ...
// >>> with ZipFile('comment_garbage.zip', 'a') as z:
// ...     z.comment = b'short.'
// ...
// >>>
//
// Hexdump:
//
// 00000000  50 4b 05 06 00 00 00 00  00 00 00 00 00 00 00 00  |PK..............|
// 00000010  00 00 00 00 06 00 73 68  6f 72 74 2e 6f 6d 6d 65  |......short.omme|
// 00000020  6e 74 20 62 6c 61 20 62  6c 61 20 62 6c 61        |nt bla bla bla|
// 0000002e

use std::io;
use zip::ZipArchive;

#[test]
fn correctly_handle_zip_with_garbage_after_comment() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("../tests/data/comment_garbage.zip"));
    let archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip file");

    assert_eq!(archive.comment(), "short.".as_bytes());
}

/// Ensure that a file which has the signature misaligned with the window size is still
/// successfully located.
#[test]
fn correctly_handle_cde_on_window() {
    let mut v = Vec::new();
    v.extend_from_slice(include_bytes!("../tests/data/misaligned_comment.zip"));
    assert_eq!(v.len(), 512 + 1);
    let sig: [u8; 4] = v[..4].try_into().unwrap();
    let sig = u32::from_le_bytes(sig);

    const CENTRAL_DIRECTORY_END_SIGNATURE: u32 = 0x06054b50;
    assert_eq!(sig, CENTRAL_DIRECTORY_END_SIGNATURE);

    let archive = ZipArchive::new(io::Cursor::new(v)).expect("couldn't open test zip");

    assert_eq!(archive.comment(), "short.".as_bytes());
}
