#![cfg(all(
    not(miri),
    not(all(feature = "deflate-zopfli", not(feature = "deflate-flate2")))
))]
//! Tests for header consistency validation in ZipStreamReader.
//! Verifies mitigation for dual-header desynchronization / Zip Cloaking (CWE-436 / CWE-20).

use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::PathBuf;
use tempfile::tempdir;
use zip::result::ZipError;
use zip::unstable::stream::ZipStreamReader;
use zip::write::SimpleFileOptions;
use zip::{System, ZipArchive, ZipWriter};

/// Simulates a security gateway inspection using Central Directory (CDFH).
fn security_gateway_inspection(archive_bytes: &[u8]) -> Result<(), String> {
    let mut archive = ZipArchive::new(Cursor::new(archive_bytes))
        .map_err(|e| format!("Corrupted archive: {:?}", e))?;

    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| format!("Entry error: {:?}", e))?;
        let name = entry.name().map_err(|e| format!("Name error: {:?}", e))?;

        if name.ends_with(".sh") || name.ends_with(".exe") || name.ends_with(".bat") {
            return Err(format!("Blocked executable/script: {}", name));
        }
        if !name.ends_with(".pdf") {
            return Err(format!("Policy violation: {}", name));
        }
    }
    Ok(())
}

/// Names in `dir`, sorted.
fn dir_entries(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// Constructs a dual-header archive where LFH is "cron_worker.sh" and CDFH is "doc_report.pdf".
fn create_dual_header_archive() -> Vec<u8> {
    const SCRIPT_NAME: &str = "cron_worker.sh";
    const SPOOFED_NAME: &str = "doc_report.pdf";
    const CENTRAL_HEADER_BASE_SIZE: usize = 46;

    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);
    let options = SimpleFileOptions::default();

    writer.start_file(SCRIPT_NAME, options).unwrap();
    writer
        .write_all(b"#!/bin/sh\necho 'MALICIOUS SCRIPT EXECUTED'\n")
        .unwrap();
    writer.finish().unwrap();

    let mut archive_bytes = payload.into_inner();
    let cdh = archive_bytes
        .windows(4)
        .position(|s| s == b"PK\x01\x02")
        .expect("Central Directory Header signature must exist");

    let cdh_filename_offset = cdh + CENTRAL_HEADER_BASE_SIZE;
    assert_eq!(
        &archive_bytes[cdh_filename_offset..cdh_filename_offset + SCRIPT_NAME.len()],
        SCRIPT_NAME.as_bytes()
    );
    archive_bytes[cdh_filename_offset..cdh_filename_offset + SCRIPT_NAME.len()]
        .copy_from_slice(SPOOFED_NAME.as_bytes());

    archive_bytes
}

#[test]
fn test_legacy_behavior_without_header_consistency_enforcement() {
    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();
    let target_script = dest_dir.join("cron_worker.sh");
    fs::write(&target_script, b"#!/bin/sh\necho 'SERVICE HEALTHY'\n").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target_script, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let archive_bytes = create_dual_header_archive();

    // Gateway inspection sees "doc_report.pdf" and approves
    assert!(security_gateway_inspection(&archive_bytes).is_ok());

    // Default extraction extracts "cron_worker.sh" based on Local File Header
    let reader = ZipStreamReader::new(Cursor::new(&archive_bytes));
    assert!(reader.extract(dest_dir).is_ok());

    let overwritten_content = fs::read_to_string(&target_script).unwrap();
    assert!(overwritten_content.contains("MALICIOUS SCRIPT EXECUTED"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::metadata(&target_script).unwrap().permissions();
        assert_eq!(
            perms.mode() & 0o111,
            0o111,
            "Executable bits should be preserved on overwritten target"
        );
    }
}

#[test]
fn test_enforce_header_consistency_blocks_dual_header_mismatch() {
    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();
    let target_script = dest_dir.join("cron_worker.sh");
    fs::write(&target_script, b"#!/bin/sh\necho 'SERVICE HEALTHY'\n").unwrap();

    let archive_bytes = create_dual_header_archive();

    assert!(security_gateway_inspection(&archive_bytes).is_ok());

    // When enforce_header_consistency is enabled, mismatch is detected and rejected
    let reader = ZipStreamReader::new(Cursor::new(&archive_bytes)).enforce_header_consistency(true);
    let result = reader.extract(dest_dir);

    assert!(result.is_err());
    match result {
        Err(ZipError::InvalidArchive(msg)) => {
            assert!(msg.contains("inconsistency") || msg.contains("mismatch"));
        }
        other => panic!("Expected ZipError::InvalidArchive, got: {:?}", other),
    }
    // Nothing was written before the mismatch was found.
    assert_eq!(
        fs::read_to_string(&target_script).unwrap(),
        "#!/bin/sh\necho 'SERVICE HEALTHY'\n"
    );
    assert_eq!(dir_entries(dest_dir), ["cron_worker.sh"]);
}

#[test]
fn test_enforce_header_consistency_allows_valid_archive() {
    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();
    fs::write(dest_dir.join("data.txt"), b"old").unwrap();

    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);
    let options = SimpleFileOptions::default();

    writer.start_file("document.pdf", options).unwrap();
    writer.write_all(b"%PDF-1.7 Test Document Content").unwrap();

    writer.start_file("data.txt", options).unwrap();
    writer.write_all(b"Sample plain text").unwrap();
    writer.finish().unwrap();

    let valid_zip = payload.into_inner();

    let reader = ZipStreamReader::new(Cursor::new(&valid_zip)).enforce_header_consistency(true);
    assert!(reader.extract(dest_dir).is_ok());

    let doc_content = fs::read_to_string(dest_dir.join("document.pdf")).unwrap();
    let txt_content = fs::read_to_string(dest_dir.join("data.txt")).unwrap();
    assert_eq!(doc_content, "%PDF-1.7 Test Document Content");
    assert_eq!(txt_content, "Sample plain text");
    assert_eq!(dir_entries(dest_dir), ["data.txt", "document.pdf"]);
}

#[test]
fn test_enforce_header_consistency_extracts_nested_entries() {
    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();

    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);
    let options = SimpleFileOptions::default().unix_permissions(0o600);
    writer.add_directory("empty/", options).unwrap();
    writer.start_file("a/b/file.txt", options).unwrap();
    writer.write_all(b"nested").unwrap();
    writer.finish().unwrap();

    let reader =
        ZipStreamReader::new(Cursor::new(payload.into_inner())).enforce_header_consistency(true);
    reader.extract(dest_dir).unwrap();

    assert_eq!(dir_entries(dest_dir), ["a", "empty"]);
    assert!(dest_dir.join("empty").is_dir());
    let file = dest_dir.join("a/b/file.txt");
    assert_eq!(fs::read_to_string(&file).unwrap(), "nested");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn test_stream_extract_keeps_read_only_flag() {
    for enforce in [false, true] {
        let temp_env = tempdir().unwrap();
        let dest_dir = temp_env.path();

        let mut payload = Cursor::new(Vec::new());
        let mut writer = ZipWriter::new(&mut payload);
        let options = SimpleFileOptions::default()
            .system(System::Unix)
            .unix_permissions(0o444);
        writer.start_file("a/file.txt", options).unwrap();
        writer.write_all(b"read-only").unwrap();
        writer.finish().unwrap();

        let reader = ZipStreamReader::new(Cursor::new(payload.into_inner()))
            .enforce_header_consistency(enforce);
        reader.extract(dest_dir).unwrap();

        let file = dest_dir.join("a/file.txt");
        assert_eq!(fs::read_to_string(&file).unwrap(), "read-only");
        assert!(fs::metadata(&file).unwrap().permissions().readonly());
    }
}

/// An archive that, when first read, names its entry after the only directory in `dest`: the
/// staging directory, which `extract` creates before reading.
struct ArchiveNamedAfterStagingDir {
    dest: PathBuf,
    archive: Option<Cursor<Vec<u8>>>,
}

impl Read for ArchiveNamedAfterStagingDir {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let dest = &self.dest;
        let archive = self.archive.get_or_insert_with(|| {
            let [staging] = &dir_entries(dest)[..] else {
                panic!("expected only the staging directory in {dest:?}");
            };
            let mut payload = Cursor::new(Vec::new());
            let mut writer = ZipWriter::new(&mut payload);
            writer
                .start_file(format!("{staging}/file.txt"), SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"collides").unwrap();
            writer.finish().unwrap();
            Cursor::new(payload.into_inner())
        });
        archive.read(buf)
    }
}

#[test]
fn test_enforce_header_consistency_extracts_entry_named_like_staging_dir() {
    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();

    let archive = ArchiveNamedAfterStagingDir {
        dest: dest_dir.to_owned(),
        archive: None,
    };
    let reader = ZipStreamReader::new(archive).enforce_header_consistency(true);
    reader.extract(dest_dir).unwrap();

    let [name] = &dir_entries(dest_dir)[..] else {
        panic!("expected only the entry's directory in {dest_dir:?}");
    };
    let file = dest_dir.join(name).join("file.txt");
    assert_eq!(fs::read_to_string(file).unwrap(), "collides");
}

/// Adversarial Test: Name Swapping via Offset Swapping in Central Directory
/// Entry 0 in stream is "file_a.txt" at offset 0
/// Entry 1 in stream is "file_b.txt" at offset X
/// In CDE, entry 0 claims to be "file_a.txt" but sets offset = X
/// In CDE, entry 1 claims to be "file_b.txt" but sets offset = 0
#[test]
fn test_enforce_header_consistency_blocks_name_swapping_via_offsets() {
    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();

    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);
    let options = SimpleFileOptions::default();

    writer.start_file("file_a.txt", options).unwrap();
    writer.write_all(b"CONTENT OF FILE A").unwrap();

    writer.start_file("file_b.txt", options).unwrap();
    writer.write_all(b"CONTENT OF FILE B").unwrap();
    writer.finish().unwrap();

    let mut archive_bytes = payload.into_inner();

    // Find the two Central Directory Headers ("PK\x01\x02")
    let cde_indices: Vec<usize> = archive_bytes
        .windows(4)
        .enumerate()
        .filter_map(|(i, s)| if s == b"PK\x01\x02" { Some(i) } else { None })
        .collect();
    assert_eq!(cde_indices.len(), 2);

    let cde0 = cde_indices[0];
    let cde1 = cde_indices[1];

    // Offset in CDE is at +42 (u32, little-endian)
    let offset0 = u32::from_le_bytes(archive_bytes[cde0 + 42..cde0 + 46].try_into().unwrap());
    let offset1 = u32::from_le_bytes(archive_bytes[cde1 + 42..cde1 + 46].try_into().unwrap());

    // Swap the offsets: CDE entry 0 ("file_a.txt") now points to offset1 ("file_b.txt")
    // CDE entry 1 ("file_b.txt") now points to offset0 ("file_a.txt")
    archive_bytes[cde0 + 42..cde0 + 46].copy_from_slice(&offset1.to_le_bytes());
    archive_bytes[cde1 + 42..cde1 + 46].copy_from_slice(&offset0.to_le_bytes());

    let reader = ZipStreamReader::new(Cursor::new(&archive_bytes)).enforce_header_consistency(true);
    let result = reader.extract(dest_dir);

    assert!(
        result.is_err(),
        "Name swap via offset redirection MUST be detected and rejected!"
    );
}

/// Adversarial Test: CRC32 mismatch between Local File Header and Central Directory
#[test]
fn test_enforce_header_consistency_blocks_crc_tampering() {
    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();

    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);
    let options = SimpleFileOptions::default();

    writer.start_file("test.txt", options).unwrap();
    writer.write_all(b"INTEGRITY PROTECTED DATA").unwrap();
    writer.finish().unwrap();

    let mut archive_bytes = payload.into_inner();

    // Local File Header signature is at offset 0 ("PK\x03\x04")
    // CRC32 is at LFH offset +14 (4 bytes)
    assert_eq!(&archive_bytes[0..4], b"PK\x03\x04");
    // Corrupt CRC32 in LFH
    archive_bytes[14..18].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());

    let reader = ZipStreamReader::new(Cursor::new(&archive_bytes)).enforce_header_consistency(true);
    let result = reader.extract(dest_dir);

    assert!(
        result.is_err(),
        "CRC32 mismatch between LFH and CDE MUST be rejected!"
    );
}

/// Adversarial Test: Size discrepancy between Local File Header and Central Directory.
/// A crafted file where the 5-byte payload b"HELLO" shares the identical CRC32 (0xc1446436)
/// with the 9-byte extended sequence b"HELLO\x5c\x0e\x0d\x24".
/// Both LFH and CDFH have matching CRC32, but CDFH advertises uncompressed size 9 while LFH has 5.
/// Without header consistency checks, the 5-byte file extracts cleanly with valid CRC32.
/// With enforce_header_consistency, the size desynchronization is caught and rejected.
#[test]
fn test_enforce_header_consistency_blocks_size_truncation() {
    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();

    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    const PREFIX_PAYLOAD: &[u8] = b"HELLO";
    const FULL_PAYLOAD: &[u8] = b"HELLO\x5c\x0e\x0d\x24";
    // Mathematically prove CRC32 collision between prefix and full payload
    assert_eq!(
        crc32fast::hash(PREFIX_PAYLOAD),
        crc32fast::hash(FULL_PAYLOAD)
    );

    writer.start_file("trunc.txt", options).unwrap();
    writer.write_all(PREFIX_PAYLOAD).unwrap();
    writer.finish().unwrap();

    let mut archive_bytes = payload.into_inner();

    // Locate Central Directory Header ("PK\x01\x02")
    let cde_pos = archive_bytes
        .windows(4)
        .position(|s| s == b"PK\x01\x02")
        .expect("Central Directory Header must exist");

    // In CDFH: uncompressed size is at offset +24 (4 bytes)
    // Tamper CDFH uncompressed size to 9 (matching FULL_PAYLOAD length, sharing same CRC32)
    archive_bytes[cde_pos + 24..cde_pos + 28].copy_from_slice(&9u32.to_le_bytes());

    let reader = ZipStreamReader::new(Cursor::new(&archive_bytes)).enforce_header_consistency(true);
    let result = reader.extract(dest_dir);

    assert!(
        result.is_err(),
        "Size discrepancy between LFH and CDE MUST be rejected!"
    );
    match result {
        Err(ZipError::InvalidArchive(msg)) => {
            assert!(
                msg.contains("uncompressed size mismatch"),
                "Expected uncompressed size mismatch error, got: {}",
                msg
            );
        }
        other => panic!("Expected ZipError::InvalidArchive, got: {:?}", other),
    }
}

/// Adversarial Test: UnicodePath (0x7075) extra field desynchronization
#[test]
fn test_enforce_header_consistency_blocks_upath_mismatch() {
    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();

    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);

    let mut upath_data = Vec::new();
    upath_data.push(1u8); // version 1
    let crc = crc32fast::hash(b"safe.txt");
    upath_data.extend_from_slice(&crc.to_le_bytes());
    upath_data.extend_from_slice(b"evil.sh");

    let mut options = SimpleFileOptions::default().into_full_options();
    // Add 0x7075 to Central Directory ONLY to desynchronize resolved name with local header
    options.add_extra_field(0x7075, upath_data, true).unwrap();

    writer.start_file("safe.txt", options).unwrap();
    writer.write_all(b"SAFE CONTENT").unwrap();
    writer.finish().unwrap();

    let archive_bytes = payload.into_inner();

    let reader = ZipStreamReader::new(Cursor::new(&archive_bytes)).enforce_header_consistency(true);
    let result = reader.extract(dest_dir);

    assert!(
        result.is_err(),
        "UnicodePath (0x7075) desync MUST be detected and rejected!"
    );
    match result {
        Err(ZipError::InvalidArchive(msg)) => {
            assert!(msg.contains("UnicodePath") || msg.contains("mismatch"));
        }
        other => panic!("Expected ZipError::InvalidArchive, got: {:?}", other),
    }
}

/// Adversarial Test: Compression method mismatch (Zombie ZIP / Evasion bypass)
/// Reference: https://labs.cloudsecurityalliance.org/research/csa-research-note-zombie-zip-archive-evasion-av-bypass-20260/
/// Written as compressed payload, then reinterpreted as Stored in the Local File Header.
/// AV / security gateways inspecting CDFH decompress and validate a legitimate Deflate stream,
/// while streaming readers see Stored and extract the raw payload without decompression.
#[test]
fn test_enforce_header_consistency_blocks_compression_method_mismatch() {
    if zip::CompressionMethod::default() == zip::CompressionMethod::Stored {
        // No compression algorithm feature active; skip test
        return;
    }

    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();

    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);
    let options = SimpleFileOptions::default();

    writer.start_file("zombie_payload.bin", options).unwrap();
    writer
        .write_all(b"COMPRESSED MALICIOUS EXECUTABLE PAYLOAD")
        .unwrap();
    writer.finish().unwrap();

    let mut archive_bytes = payload.into_inner();

    // Locate Central Directory Header ("PK\x01\x02")
    let cde_pos = archive_bytes
        .windows(4)
        .position(|s| s == b"PK\x01\x02")
        .expect("Central Directory Header must exist");

    // In Central Directory Header, compression method is at offset +10 (2 bytes)
    // Reinterpret as Stored (0) in CDFH while Local File Header retains Deflated
    archive_bytes[cde_pos + 10..cde_pos + 12].copy_from_slice(&0u16.to_le_bytes());

    let reader = ZipStreamReader::new(Cursor::new(&archive_bytes)).enforce_header_consistency(true);
    let result = reader.extract(dest_dir);

    assert!(
        result.is_err(),
        "Compression method mismatch MUST be rejected!"
    );
    match result {
        Err(ZipError::InvalidArchive(msg)) => {
            assert!(
                msg.contains("compression method mismatch"),
                "Expected compression method mismatch error, got: {}",
                msg
            );
        }
        other => panic!("Expected ZipError::InvalidArchive, got: {:?}", other),
    }
}

/// Adversarial Test: Entry count mismatch and premature Central Directory boundary
#[test]
fn test_enforce_header_consistency_blocks_entry_count_mismatch() {
    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();

    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);
    let options = SimpleFileOptions::default();

    writer.start_file("entry1.txt", options).unwrap();
    writer.write_all(b"DATA 1").unwrap();
    writer.start_file("entry2.txt", options).unwrap();
    writer.write_all(b"DATA 2").unwrap();
    writer.finish().unwrap();

    let mut archive_bytes = payload.into_inner();

    // Find the second CDE ("PK\x01\x02") and corrupt its signature to "PK\x05\x06" (EOCD),
    // causing central directory parsing to terminate after 1 entry while stream has 2 entries.
    let cde_indices: Vec<usize> = archive_bytes
        .windows(4)
        .enumerate()
        .filter_map(|(i, s)| if s == b"PK\x01\x02" { Some(i) } else { None })
        .collect();
    assert_eq!(cde_indices.len(), 2);

    let cde1 = cde_indices[1];
    // Overwrite second CDE signature to simulate premature end
    archive_bytes[cde1..cde1 + 4].copy_from_slice(b"PK\x05\x06");

    let reader = ZipStreamReader::new(Cursor::new(&archive_bytes)).enforce_header_consistency(true);
    let result = reader.extract(dest_dir);

    assert!(
        result.is_err(),
        "Entry count mismatch (2 local vs 1 central) MUST be rejected!"
    );
    match result {
        Err(ZipError::InvalidArchive(msg)) => {
            assert!(msg.contains("entry count mismatch"));
        }
        other => panic!("Expected ZipError::InvalidArchive, got: {:?}", other),
    }
    assert_eq!(dir_entries(dest_dir), [""; 0]);
}

/// Adversarial Test: Central entry offset pointing inside previous entry's filename/payload
/// Simulates an archive where CDE header_start points into a fake header within entry 0's filename/comment.
#[test]
fn test_enforce_header_consistency_blocks_offset_pointing_into_previous_entry() {
    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();

    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);
    let options = SimpleFileOptions::default();

    // Entry 0 with long payload/name
    writer.start_file("first_long_entry.txt", options).unwrap();
    writer
        .write_all(b"SOME PAYLOAD DATA HERE FOR FIRST ENTRY")
        .unwrap();

    writer.start_file("second_entry.txt", options).unwrap();
    writer.write_all(b"SECOND ENTRY CONTENT").unwrap();
    writer.finish().unwrap();

    let mut archive_bytes = payload.into_inner();

    // Find the second Central Directory Header ("PK\x01\x02")
    let cde_indices: Vec<usize> = archive_bytes
        .windows(4)
        .enumerate()
        .filter_map(|(i, s)| if s == b"PK\x01\x02" { Some(i) } else { None })
        .collect();
    assert_eq!(cde_indices.len(), 2);

    let cde1 = cde_indices[1];
    // Tamper second CDE offset (+42) to point to offset 10 (inside first entry's header/filename)
    archive_bytes[cde1 + 42..cde1 + 46].copy_from_slice(&10u32.to_le_bytes());

    let reader = ZipStreamReader::new(Cursor::new(&archive_bytes)).enforce_header_consistency(true);
    let result = reader.extract(dest_dir);

    assert!(
        result.is_err(),
        "Offset pointing into previous entry MUST be rejected!"
    );
    match result {
        Err(ZipError::InvalidArchive(msg)) => {
            assert!(
                msg.contains("offset mismatch")
                    || msg.contains("overlaps")
                    || msg.contains("inconsistency")
            );
        }
        other => panic!("Expected ZipError::InvalidArchive, got: {:?}", other),
    }
}

#[test]
fn test_ziparchive_validate_header_consistency_detects_dual_header_mismatch() {
    let archive_bytes = create_dual_header_archive();
    let mut archive = ZipArchive::new(Cursor::new(&archive_bytes)).unwrap();

    // Full archive scan detects filename mismatch
    let result = archive.validate_header_consistency();
    assert!(
        result.is_err(),
        "validate_header_consistency MUST detect LFH vs CDFH mismatch"
    );
    match result {
        Err(ZipError::InvalidArchive(msg)) => {
            assert!(msg.contains("does not match central directory"));
        }
        other => panic!("Expected ZipError::InvalidArchive, got: {:?}", other),
    }

    // by_index_with_options with enforce_header_consistency=true fails
    let opt_enforced = zip::ZipReadOptions::new().enforce_header_consistency(true);
    assert!(
        archive.by_index_with_options(0, opt_enforced).is_err(),
        "by_index_with_options with enforce_header_consistency=true MUST fail"
    );

    // by_name_with_options with enforce_header_consistency=true fails
    let opt_enforced2 = zip::ZipReadOptions::new().enforce_header_consistency(true);
    assert!(
        archive
            .by_name_with_options("doc_report.pdf", opt_enforced2)
            .is_err(),
        "by_name_with_options with enforce_header_consistency=true MUST fail"
    );

    // Legacy by_index without enforcement still opens for backwards compatibility
    assert!(archive.by_index(0).is_ok());
}

#[test]
fn test_ziparchive_validate_header_consistency_passes_on_valid_archive() {
    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);
    let options = SimpleFileOptions::default();

    writer.start_file("valid1.txt", options).unwrap();
    writer.write_all(b"Hello world 1").unwrap();
    writer.start_file("valid2.txt", options).unwrap();
    writer.write_all(b"Hello world 2").unwrap();
    writer.finish().unwrap();

    let archive_bytes = payload.into_inner();
    let mut archive = ZipArchive::new(Cursor::new(&archive_bytes)).unwrap();

    // Validation passes completely
    assert!(archive.validate_header_consistency().is_ok());

    // by_index_with_options passes
    let opt = zip::ZipReadOptions::new().enforce_header_consistency(true);
    assert!(archive.by_index_with_options(0, opt).is_ok());

    // by_name_with_options passes
    let opt2 = zip::ZipReadOptions::new().enforce_header_consistency(true);
    assert!(archive.by_name_with_options("valid2.txt", opt2).is_ok());
}

#[test]
fn test_ziparchive_validate_header_consistency_detects_zip64_size_mismatch() {
    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .large_file(true);

    writer.start_file("zip64_test.txt", options).unwrap();
    writer.write_all(b"Hello ZIP64 data").unwrap();
    writer.finish().unwrap();

    let mut archive_bytes = payload.into_inner();

    // Verify valid ZIP64 archive passes consistency check
    let mut archive = ZipArchive::new(Cursor::new(&archive_bytes)).unwrap();
    assert!(archive.validate_header_consistency().is_ok());

    // Locate Central Directory Header ('PK\x01\x02')
    let cdh = archive_bytes
        .windows(4)
        .position(|s| s == b"PK\x01\x02")
        .expect("CDFH signature must exist");

    // In CDFH, extra field follows filename. Find ZIP64 extra field (tag 0x0001)
    let extra_offset = cdh + 46 + "zip64_test.txt".len();
    assert_eq!(
        &archive_bytes[extra_offset..extra_offset + 2],
        &1u16.to_le_bytes()
    );
    // ZIP64 sizes block starts at extra_offset + 4 (tag 2B + len 2B)
    // First 8 bytes are uncompressed size
    let uncomp_size_offset = extra_offset + 4;
    let original_size = u64::from_le_bytes(
        archive_bytes[uncomp_size_offset..uncomp_size_offset + 8]
            .try_into()
            .unwrap(),
    );
    assert_eq!(original_size, 16);

    // Tamper with central directory ZIP64 uncompressed size (change to 9999)
    archive_bytes[uncomp_size_offset..uncomp_size_offset + 8]
        .copy_from_slice(&9999u64.to_le_bytes());

    let mut tampered_archive = ZipArchive::new(Cursor::new(&archive_bytes)).unwrap();
    let result = tampered_archive.validate_header_consistency();
    assert!(
        result.is_err(),
        "validate_header_consistency MUST detect ZIP64 size discrepancy"
    );
    match result {
        Err(ZipError::InvalidArchive(msg)) => {
            assert!(
                msg.contains("uncompressed size mismatch"),
                "Unexpected error: {}",
                msg
            );
        }
        other => panic!("Expected ZipError::InvalidArchive, got: {:?}", other),
    }
}

#[test]
fn test_enforce_header_consistency_blocks_encryption_flag_mismatch() {
    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();

    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    writer.start_file("plain.txt", options).unwrap();
    writer.write_all(b"PLAINTEXT PAYLOAD").unwrap();
    writer.finish().unwrap();

    let mut archive_bytes = payload.into_inner();

    // Locate Central Directory Header ("PK\x01\x02")
    let cdh = archive_bytes
        .windows(4)
        .position(|s| s == b"PK\x01\x02")
        .expect("CDFH signature must exist");

    // In CDFH, general purpose bit flags are at offset +8 (2 bytes)
    // Flip bit 0 (Encrypted) in Central Directory while Local Header remains unencrypted (0)
    let flags = u16::from_le_bytes(archive_bytes[cdh + 8..cdh + 10].try_into().unwrap());
    archive_bytes[cdh + 8..cdh + 10].copy_from_slice(&(flags | 0x0001).to_le_bytes());

    let reader = ZipStreamReader::new(Cursor::new(&archive_bytes)).enforce_header_consistency(true);
    let stream_res = reader.extract(dest_dir);
    assert!(
        stream_res.is_err(),
        "Encryption flag mismatch MUST be rejected by ZipStreamReader"
    );

    let mut archive = ZipArchive::new(Cursor::new(&archive_bytes)).unwrap();
    let archive_res = archive.validate_header_consistency();
    assert!(
        archive_res.is_err(),
        "Encryption flag mismatch MUST be rejected by ZipArchive::validate_header_consistency"
    );
}

#[test]
fn test_enforce_header_consistency_blocks_last_modified_time_mismatch() {
    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();

    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);
    let options = SimpleFileOptions::default();

    writer.start_file("timestamped.txt", options).unwrap();
    writer.write_all(b"DATA").unwrap();
    writer.finish().unwrap();

    let mut archive_bytes = payload.into_inner();

    // Locate Central Directory Header ("PK\x01\x02")
    let cdh = archive_bytes
        .windows(4)
        .position(|s| s == b"PK\x01\x02")
        .expect("CDFH signature must exist");

    // In CDFH, last_mod_time is at offset +12 (2 bytes)
    // Tamper with last_mod_time in CDFH (used as ZipCrypto password verifier when data descriptor is active)
    let orig_time = u16::from_le_bytes(archive_bytes[cdh + 12..cdh + 14].try_into().unwrap());
    archive_bytes[cdh + 12..cdh + 14].copy_from_slice(&(orig_time ^ 0x0800).to_le_bytes());

    let reader = ZipStreamReader::new(Cursor::new(&archive_bytes)).enforce_header_consistency(true);
    assert!(reader.extract(dest_dir).is_err());

    let mut archive = ZipArchive::new(Cursor::new(&archive_bytes)).unwrap();
    assert!(archive.validate_header_consistency().is_err());
}

#[cfg(feature = "aes-crypto")]
#[test]
fn test_aes_finish_into_readable_and_outer_compression_consistency() {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .with_aes_encryption(zip::AesMode::Aes256, "secret_password");

    writer.start_file("secret.txt", options).unwrap();
    writer
        .write_all(b"Sensitive payload long enough for AE-1 check")
        .unwrap();

    // Verify finish_into_readable() aligns with validate_header_consistency()
    let mut archive = writer.finish_into_readable().unwrap();
    assert!(archive.validate_header_consistency().is_ok());

    let mut archive_bytes = archive.into_inner().into_inner();

    // Tamper with Local File Header outer compression_method (offset +8, 2 bytes):
    // Change from 99 (AES) to 0 (Stored) while keeping the 0x9901 AeXEncryption extra field
    assert_eq!(
        u16::from_le_bytes(archive_bytes[8..10].try_into().unwrap()),
        99
    );
    archive_bytes[8..10].copy_from_slice(&0u16.to_le_bytes());

    let mut tampered = ZipArchive::new(Cursor::new(&archive_bytes)).unwrap();
    let res = tampered.validate_header_consistency();
    assert!(
        res.is_err(),
        "Outer compression method mismatch on AES entry MUST be rejected"
    );
}

#[test]
fn test_upath_valid_and_raw_name_smuggling_blocked() {
    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();

    // 1. Valid archive with identical 0x7075 UnicodePath in both LFH and CDFH must pass
    let mut upath_data = vec![1u8];
    upath_data.extend_from_slice(&crc32fast::hash(b"fallback.txt").to_le_bytes());
    upath_data.extend_from_slice("ümlaut.txt".as_bytes());

    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);
    let mut options = SimpleFileOptions::default().into_full_options();
    options
        .add_extra_field(0x7075, upath_data.clone(), false)
        .unwrap();
    writer.start_file("fallback.txt", options).unwrap();
    writer.write_all(b"UTF8 PATH CONTENT").unwrap();
    writer.finish().unwrap();

    let mut archive_bytes = payload.into_inner();

    let mut archive = ZipArchive::new(Cursor::new(&archive_bytes)).unwrap();
    assert!(
        archive.validate_header_consistency().is_ok(),
        "Valid 0x7075 UnicodePath in both headers must passZipArchive::validate_header_consistency"
    );
    let reader = ZipStreamReader::new(Cursor::new(&archive_bytes)).enforce_header_consistency(true);
    assert!(reader.extract(dest_dir).is_ok());

    // 2. Tamper LFH raw name ("evil_cmd.sh\0") and LFH 0x7075 CRC32 to match "evil_cmd.sh\0"
    // while keeping 0x7075 content ("ümlaut.txt") identical in both headers.
    let spoofed_raw = b"evil_cmd.sh!";
    assert_eq!(spoofed_raw.len(), b"fallback.txt".len());
    archive_bytes[30..42].copy_from_slice(spoofed_raw);
    let spoofed_crc = crc32fast::hash(spoofed_raw);
    // In LFH, extra field starts at 42: tag (2B) + len (2B) + version (1B) + crc32 (4B at 47..51)
    archive_bytes[47..51].copy_from_slice(&spoofed_crc.to_le_bytes());

    let mut tampered_archive = ZipArchive::new(Cursor::new(&archive_bytes)).unwrap();
    assert!(
        tampered_archive.validate_header_consistency().is_err(),
        "Raw filename mismatch hidden behind identical 0x7075 content MUST be rejected by ZipArchive"
    );

    let tampered_stream =
        ZipStreamReader::new(Cursor::new(&archive_bytes)).enforce_header_consistency(true);
    assert!(
        tampered_stream.extract(dest_dir).is_err(),
        "Raw filename mismatch hidden behind identical 0x7075 content MUST be rejected by ZipStreamReader"
    );
}

#[test]
fn test_duplicate_filename_shadowing_blocked() {
    let temp_env = tempdir().unwrap();
    let dest_dir = temp_env.path();

    let mut payload = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(&mut payload);
    let options = SimpleFileOptions::default();

    // Entry 0: "cron_worker.sh" (will be spoofed in CDFH to "benign_doc.pdf", colliding with Entry 1)
    writer.start_file("cron_worker.sh", options).unwrap();
    writer.write_all(b"#!/bin/sh\necho PWNED\n").unwrap();

    // Entry 1: "benign_doc.pdf" (same length as "cron_worker.sh")
    writer.start_file("benign_doc.pdf", options).unwrap();
    writer.write_all(b"%PDF-1.7 HARMLESS\n").unwrap();
    writer.finish().unwrap();

    let mut archive_bytes = payload.into_inner();

    // Spoof Entry 0's CDFH filename from "cron_worker.sh" to "benign_doc.pdf"
    // so Entry 1 evicts Entry 0 in ZipArchive's IndexMap!
    let cdh0 = archive_bytes
        .windows(4)
        .position(|s| s == b"PK\x01\x02")
        .unwrap();
    archive_bytes[cdh0 + 46..cdh0 + 46 + 14].copy_from_slice(b"benign_doc.pdf");

    let mut archive = ZipArchive::new(Cursor::new(&archive_bytes)).unwrap();
    assert!(
        archive.validate_header_consistency().is_err(),
        "Duplicate filename shadowing in CDFH MUST be rejected by validate_header_consistency"
    );
    let opt = zip::ZipReadOptions::new().enforce_header_consistency(true);
    assert!(
        archive.by_name_with_options("benign_doc.pdf", opt).is_err(),
        "Duplicate filename shadowing MUST be rejected by by_name_with_options when enforce_header_consistency is enabled"
    );

    let reader = ZipStreamReader::new(Cursor::new(&archive_bytes)).enforce_header_consistency(true);
    assert!(reader.extract(dest_dir).is_err());
}

/// Implode flag bits 1-2 set the window size and tree count; a local/central mismatch makes
/// `ZipArchive` and the stream reader decode different bytes.
#[test]
fn test_enforce_header_consistency_blocks_implode_flag_mismatch() {
    use std::io::Read;

    let temp_env = tempdir().unwrap();

    for mask in [0x0002u16, 0x0004] {
        let mut archive_bytes = include_bytes!("data/legacy/implode.zip").to_vec();
        let cdh = archive_bytes
            .windows(4)
            .position(|s| s == b"PK\x01\x02")
            .expect("CDFH signature must exist");
        // Central directory flags only; the local header keeps the original bits.
        let flags = u16::from_le_bytes(archive_bytes[cdh + 8..cdh + 10].try_into().unwrap());
        archive_bytes[cdh + 8..cdh + 10].copy_from_slice(&(flags ^ mask).to_le_bytes());

        let mut archive = ZipArchive::new(Cursor::new(&archive_bytes)).unwrap();
        assert!(
            archive.validate_header_consistency().is_err(),
            "Implode flag mismatch {mask:#06x} MUST be rejected by validate_header_consistency"
        );
        let opt = zip::ZipReadOptions::new().enforce_header_consistency(true);
        let read = archive.by_index_with_options(0, opt).map(|mut f| {
            let mut v = Vec::new();
            f.read_to_end(&mut v).map(|_| v.len())
        });
        assert!(
            read.is_err(),
            "Implode flag mismatch {mask:#06x} MUST be rejected before decoding, got {read:?}"
        );

        let reader =
            ZipStreamReader::new(Cursor::new(&archive_bytes)).enforce_header_consistency(true);
        assert!(
            reader
                .extract(temp_env.path().join(mask.to_string()))
                .is_err(),
            "Implode flag mismatch {mask:#06x} MUST be rejected by ZipStreamReader"
        );
    }
}
