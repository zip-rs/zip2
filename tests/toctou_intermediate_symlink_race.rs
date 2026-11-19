//! Regression tests for GHSA-h3x5-6x3j-3qm9 and GHSA-9657-5g7m-q4pf:
//! concurrent intermediate-directory symlink/junction swap (TOCTOU) during extraction.
#![cfg(all(any(unix, windows), not(miri)))]

use std::io::{Cursor, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tempfile::TempDir;
use zip::unstable::stream::ZipStreamReader;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const PAYLOAD: &[u8] = b"TOCTOU_ESCAPED_PAYLOAD";
const N_ENTRIES: usize = 40;
const MIN_RACE_EXTRACTS: u64 = 20;

fn toctou_budget_ms(default_ms: u128) -> u128 {
    std::env::var("TOCTOU_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default_ms)
}

fn maybe_yield() {
    if rand::random::<bool>() {
        std::thread::yield_now();
    }
}

fn supports_dir_links(jail_dir: &Path, target: &Path) -> bool {
    let probe = jail_dir.join("link_probe");
    if create_dir_link(target, &probe).is_err() {
        return false;
    }
    let _ = std::fs::remove_dir(&probe);
    let _ = std::fs::remove_file(&probe);
    true
}

fn spawn_dir_racer(
    stop: Arc<AtomicBool>,
    flips: Arc<AtomicU64>,
    target: std::path::PathBuf,
    link: std::path::PathBuf,
    remove_all: bool,
    restore_real_dir: bool,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            if remove_all {
                let _ = std::fs::remove_dir_all(&link);
            } else {
                let _ = std::fs::remove_dir(&link);
            }
            let _ = std::fs::remove_file(&link);
            maybe_yield();
            if create_dir_link(&target, &link).is_ok() {
                flips.fetch_add(1, Ordering::Relaxed);
            }
            maybe_yield();
            let _ = std::fs::remove_file(&link);
            if remove_all {
                let _ = std::fs::remove_dir_all(&link);
            } else {
                let _ = std::fs::remove_dir(&link);
            }
            if restore_real_dir {
                let _ = std::fs::create_dir(&link);
            }
            maybe_yield();
        }
    })
}

fn run_race_loop(budget_ms: u128, flips: &AtomicU64, mut step: impl FnMut() -> bool) -> (u64, u64) {
    let start = std::time::Instant::now();
    let max_wait_ms = budget_ms.saturating_mul(4).max(15_000);
    let mut total_extracts = 0u64;
    let mut raced_extracts = 0u64;
    let mut prev_start_flips: Option<u64> = None;

    while raced_extracts < MIN_RACE_EXTRACTS || start.elapsed().as_millis() <= budget_ms {
        if let Some(prev) = prev_start_flips
            && flips.load(Ordering::Relaxed) == prev
        {
            std::thread::yield_now();
            let wait_start = std::time::Instant::now();
            while flips.load(Ordering::Relaxed) == prev
                && wait_start.elapsed().as_millis() < 50
                && start.elapsed().as_millis() <= max_wait_ms
            {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }

        let iter_start_flips = flips.load(Ordering::Relaxed);
        prev_start_flips = Some(iter_start_flips);
        maybe_yield();

        let tripped = step();
        total_extracts += 1;

        if flips.load(Ordering::Relaxed) > iter_start_flips {
            raced_extracts += 1;
        }
        if tripped || start.elapsed().as_millis() > max_wait_ms {
            break;
        }
        maybe_yield();
    }

    (total_extracts, raced_extracts)
}

fn build_archive() -> Vec<u8> {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..N_ENTRIES {
        writer.start_file(format!("race/f{i}"), options).unwrap();
        writer.write_all(PAYLOAD).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn create_dir_link(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return Ok(());
        }
        let status = std::process::Command::new("cmd.exe")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other("mklink /J failed"))
        }
    }
}

fn payload_or_symlink_present(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if let Ok(meta) = std::fs::symlink_metadata(&p)
                && meta.file_type().is_symlink()
            {
                return true;
            }
            if p.is_dir() {
                stack.push(p);
            } else if std::fs::read(&p).map(|c| c == PAYLOAD).unwrap_or(false) {
                return true;
            }
        }
    }
    false
}

fn build_symlink_archive() -> Vec<u8> {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..N_ENTRIES {
        writer.start_file(format!("race/f{i}"), options).unwrap();
        writer.write_all(PAYLOAD).unwrap();
        writer
            .add_symlink(format!("race/link{i}"), format!("f{i}"), options)
            .unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn static_preexisting_intermediate_symlink_is_rejected() {
    let jail = TempDir::with_prefix("toctou_static").unwrap();
    let dest = jail.path().join("dest");
    let canary = jail.path().join("canary");
    std::fs::create_dir(&dest).unwrap();
    std::fs::create_dir(&canary).unwrap();

    if create_dir_link(&canary, &dest.join("race")).is_err() {
        return;
    }

    let bytes = build_archive();
    let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
    assert!(archive.extract(&dest).is_err());
    assert!(
        !payload_or_symlink_present(&canary),
        "static intermediate symlink/junction was followed into canary"
    );

    let stream_res = ZipStreamReader::new(Cursor::new(&bytes)).extract(&dest);
    assert!(stream_res.is_err());
    assert!(
        !payload_or_symlink_present(&canary),
        "static intermediate symlink/junction was followed by stream extractor into canary"
    );
}

#[test]
fn concurrent_intermediate_symlink_race_archive_extract() {
    let jail = TempDir::with_prefix("toctou_archive_jail").unwrap();
    let dest = jail.path().join("dest");
    let canary = jail.path().join("canary");
    std::fs::create_dir(&dest).unwrap();
    std::fs::create_dir(&canary).unwrap();
    if !supports_dir_links(jail.path(), &canary) {
        return;
    }

    let bytes = build_archive();
    let budget_ms = toctou_budget_ms(3_000);

    let stop = Arc::new(AtomicBool::new(false));
    let flips = Arc::new(AtomicU64::new(0));
    let race_dir = dest.join("race");
    let racer = spawn_dir_racer(
        Arc::clone(&stop),
        Arc::clone(&flips),
        canary.clone(),
        race_dir.clone(),
        true,
        true,
    );

    let mut escaped = false;
    let (extracts, raced_extracts) = run_race_loop(budget_ms, &flips, || {
        let _ = std::fs::remove_dir_all(&race_dir);
        let _ = std::fs::remove_file(&race_dir);
        maybe_yield();
        let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let _ = archive.extract(&dest);
        if payload_or_symlink_present(&canary) {
            escaped = true;
            return true;
        }
        false
    });
    stop.store(true, Ordering::Relaxed);
    let _ = racer.join();

    let total_flips = flips.load(Ordering::Relaxed);
    eprintln!(
        "TOCTOU archive race: {extracts} extractions ({raced_extracts} raced), {total_flips} flips, escaped={escaped}",
    );
    assert!(
        !escaped,
        "TOCTOU escape demonstrated: payload written into canary {canary:?} outside dest"
    );
    assert!(total_flips > 0, "racer performed zero link flips");
    assert!(raced_extracts >= MIN_RACE_EXTRACTS);
}

#[test]
fn concurrent_intermediate_symlink_race_symlink_entry() {
    let jail = TempDir::with_prefix("toctou_symlink_entry_jail").unwrap();
    let dest = jail.path().join("dest");
    let canary = jail.path().join("canary");
    std::fs::create_dir(&dest).unwrap();
    std::fs::create_dir(&canary).unwrap();
    if !supports_dir_links(jail.path(), &canary) {
        return;
    }

    let bytes = build_symlink_archive();
    let budget_ms = toctou_budget_ms(3_000);

    let stop = Arc::new(AtomicBool::new(false));
    let flips = Arc::new(AtomicU64::new(0));
    let race_dir = dest.join("race");
    let racer = spawn_dir_racer(
        Arc::clone(&stop),
        Arc::clone(&flips),
        canary.clone(),
        race_dir.clone(),
        true,
        true,
    );

    let mut escaped = false;
    let (extracts, raced_extracts) = run_race_loop(budget_ms, &flips, || {
        let _ = std::fs::remove_dir_all(&race_dir);
        let _ = std::fs::remove_file(&race_dir);
        maybe_yield();
        let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let _ = archive.extract(&dest);
        if payload_or_symlink_present(&canary) {
            escaped = true;
            return true;
        }
        false
    });
    stop.store(true, Ordering::Relaxed);
    let _ = racer.join();

    let total_flips = flips.load(Ordering::Relaxed);
    eprintln!(
        "TOCTOU symlink-entry race: {extracts} extractions ({raced_extracts} raced), {total_flips} flips, escaped={escaped}",
    );
    assert!(
        !escaped,
        "TOCTOU escape demonstrated for symlink entry: file or symlink written into canary {canary:?}"
    );
    assert!(total_flips > 0, "racer performed zero link flips");
    assert!(raced_extracts >= MIN_RACE_EXTRACTS);
}

#[test]
fn concurrent_intermediate_symlink_race_stream_extract() {
    let jail = TempDir::with_prefix("toctou_stream_jail").unwrap();
    let dest = jail.path().join("dest");
    let canary = jail.path().join("canary");
    std::fs::create_dir(&dest).unwrap();
    std::fs::create_dir(&canary).unwrap();
    if !supports_dir_links(jail.path(), &canary) {
        return;
    }

    let bytes = build_archive();
    let budget_ms = toctou_budget_ms(3_000);

    let stop = Arc::new(AtomicBool::new(false));
    let flips = Arc::new(AtomicU64::new(0));
    let race_dir = dest.join("race");
    let racer = spawn_dir_racer(
        Arc::clone(&stop),
        Arc::clone(&flips),
        canary.clone(),
        race_dir.clone(),
        true,
        true,
    );

    let mut escaped = false;
    let (extracts, raced_extracts) = run_race_loop(budget_ms, &flips, || {
        let _ = std::fs::remove_dir_all(&race_dir);
        let _ = std::fs::remove_file(&race_dir);
        maybe_yield();
        let reader = ZipStreamReader::new(Cursor::new(bytes.clone()));
        let _ = reader.extract(&dest);
        if payload_or_symlink_present(&canary) {
            escaped = true;
            return true;
        }
        false
    });
    stop.store(true, Ordering::Relaxed);
    let _ = racer.join();

    let total_flips = flips.load(Ordering::Relaxed);
    eprintln!(
        "TOCTOU stream race: {extracts} extractions ({raced_extracts} raced), {total_flips} flips, escaped={escaped}",
    );
    assert!(
        !escaped,
        "TOCTOU escape demonstrated on ZipStreamReader::extract: payload in canary {canary:?}"
    );
    assert!(total_flips > 0, "racer performed zero link flips");
    assert!(raced_extracts >= MIN_RACE_EXTRACTS);
}

#[test]
fn concurrent_intermediate_symlink_race_stream_consistent_extract() {
    let jail = TempDir::with_prefix("toctou_stream_consistent_jail").unwrap();
    let dest = jail.path().join("dest");
    let canary = jail.path().join("canary");
    std::fs::create_dir(&dest).unwrap();
    std::fs::create_dir(&canary).unwrap();
    if !supports_dir_links(jail.path(), &canary) {
        return;
    }

    let bytes = build_archive();
    let budget_ms = toctou_budget_ms(3_000);

    let stop = Arc::new(AtomicBool::new(false));
    let flips = Arc::new(AtomicU64::new(0));
    let race_dir = dest.join("race");
    let racer = spawn_dir_racer(
        Arc::clone(&stop),
        Arc::clone(&flips),
        canary.clone(),
        race_dir.clone(),
        true,
        true,
    );

    let mut escaped = false;
    let (extracts, raced_extracts) = run_race_loop(budget_ms, &flips, || {
        let _ = std::fs::remove_dir_all(&race_dir);
        let _ = std::fs::remove_file(&race_dir);
        maybe_yield();
        let reader =
            ZipStreamReader::new(Cursor::new(bytes.clone())).enforce_header_consistency(true);
        let _ = reader.extract(&dest);
        if payload_or_symlink_present(&canary) {
            escaped = true;
            return true;
        }
        false
    });
    stop.store(true, Ordering::Relaxed);
    let _ = racer.join();

    let total_flips = flips.load(Ordering::Relaxed);
    eprintln!(
        "TOCTOU stream consistent race: {extracts} extractions ({raced_extracts} raced), {total_flips} flips, escaped={escaped}",
    );
    assert!(
        !escaped,
        "TOCTOU escape demonstrated on consistent ZipStreamReader::extract: payload in canary {canary:?}"
    );
    assert!(total_flips > 0, "racer performed zero link flips");
    assert!(raced_extracts >= MIN_RACE_EXTRACTS);
}

fn build_bad_crc_archive(entry_name: &str, size: usize) -> Vec<u8> {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let payload = vec![0x41u8; size];
    writer.start_file(entry_name, options).unwrap();
    writer.write_all(&payload).unwrap();
    let mut bytes = writer.finish().unwrap().into_inner();
    // Flip a byte near the end of the stored payload (after local header + entry name)
    // so `copy_into` writes the partial file and fails CRC32 at EOF.
    let marker = vec![0x41u8; 64];
    let pos = bytes
        .windows(marker.len())
        .position(|w| w == marker.as_slice())
        .expect("stored payload marker");
    bytes[pos + size - 1] ^= 0xFF;
    bytes
}

#[test]
fn concurrent_intermediate_symlink_race_crc_error_cleanup_cannot_delete_canary() {
    let jail = TempDir::with_prefix("toctou_crc_cleanup_jail").unwrap();
    let dest = jail.path().join("dest");
    let canary = jail.path().join("canary");
    std::fs::create_dir(&dest).unwrap();
    std::fs::create_dir(&canary).unwrap();
    if !supports_dir_links(jail.path(), &canary) {
        return;
    }

    let victim = canary.join("bad.txt");
    std::fs::write(&victim, b"VICTIM_OUTSIDE_DEST").unwrap();

    let bytes = build_bad_crc_archive("race/bad.txt", 512 * 1024);
    let budget_ms = toctou_budget_ms(3_000);

    let stop = Arc::new(AtomicBool::new(false));
    let flips = Arc::new(AtomicU64::new(0));
    let race_dir = dest.join("race");
    let racer = spawn_dir_racer(
        Arc::clone(&stop),
        Arc::clone(&flips),
        canary.clone(),
        race_dir.clone(),
        true,
        false,
    );

    let mut deleted = false;
    let (extracts, raced_extracts) = run_race_loop(budget_ms, &flips, || {
        let _ = std::fs::remove_dir_all(&race_dir);
        let _ = std::fs::remove_file(&race_dir);
        maybe_yield();
        let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let res = archive.extract(&dest);
        assert!(res.is_err(), "corrupt CRC archive must fail extraction");
        if std::fs::read(&victim).ok().as_deref() != Some(b"VICTIM_OUTSIDE_DEST") {
            deleted = true;
            return true;
        }
        // Clean up any leftover partial `race/bad.txt` (e.g. after a Windows sharing
        // violation during cleanup) before `run_race_loop` waits for the next flip.
        let _ = std::fs::remove_dir_all(&race_dir);
        let _ = std::fs::remove_file(&race_dir);
        false
    });
    stop.store(true, Ordering::Relaxed);
    let _ = racer.join();

    let total_flips = flips.load(Ordering::Relaxed);
    eprintln!(
        "TOCTOU CRC cleanup race: {extracts} extractions ({raced_extracts} raced), {total_flips} flips, deleted={deleted}",
    );
    assert!(
        !deleted,
        "TOCTOU error-cleanup deleted/modified victim outside dest: {victim:?}"
    );
    assert!(total_flips > 0, "racer performed zero link flips");
    assert!(raced_extracts >= MIN_RACE_EXTRACTS);
}

#[test]
fn concurrent_intermediate_symlink_race_stream_readonly_cannot_modify_canary() {
    let jail = TempDir::with_prefix("toctou_stream_ro_jail").unwrap();
    let dest = jail.path().join("dest");
    let canary = jail.path().join("canary");
    std::fs::create_dir(&dest).unwrap();
    std::fs::create_dir(&canary).unwrap();
    if !supports_dir_links(jail.path(), &canary) {
        return;
    }

    let victim = canary.join("ro.txt");
    std::fs::write(&victim, b"CANARY").unwrap();

    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o444);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..N_ENTRIES {
        writer.start_file(format!("race/f{i}"), options).unwrap();
        writer.write_all(b"ro-payload").unwrap();
    }
    writer.start_file("race/ro.txt", options).unwrap();
    writer.write_all(b"ro-payload").unwrap();
    let bytes = writer.finish().unwrap().into_inner();

    let budget_ms = toctou_budget_ms(2_500);

    let stop = Arc::new(AtomicBool::new(false));
    let flips = Arc::new(AtomicU64::new(0));
    let race_dir = dest.join("race");
    let racer = spawn_dir_racer(
        Arc::clone(&stop),
        Arc::clone(&flips),
        canary.clone(),
        race_dir.clone(),
        true,
        false,
    );

    let mut tainted = false;
    let (extracts, raced_extracts) = run_race_loop(budget_ms, &flips, || {
        let _ = std::fs::remove_dir_all(&race_dir);
        let _ = std::fs::remove_file(&race_dir);
        maybe_yield();
        for enforce in [false, true] {
            let reader = ZipStreamReader::new(Cursor::new(bytes.clone()))
                .enforce_header_consistency(enforce);
            let _ = reader.extract(&dest);
            maybe_yield();
        }
        if std::fs::metadata(&victim)
            .map(|m| m.permissions().readonly())
            .unwrap_or(true)
        {
            tainted = true;
            return true;
        }
        false
    });
    stop.store(true, Ordering::Relaxed);
    let _ = racer.join();

    let total_flips = flips.load(Ordering::Relaxed);
    assert!(
        !tainted,
        "ZipStreamReader modified permissions on canary {victim:?} outside dest"
    );
    assert!(total_flips > 0, "racer performed zero link flips");
    assert!(
        raced_extracts >= MIN_RACE_EXTRACTS,
        "only {raced_extracts} raced extractions (out of {extracts} total, {total_flips} flips)"
    );
}

#[test]
fn preexisting_leaf_dir_link_or_readonly_hardlink_is_safe_on_overwrite() {
    let jail = TempDir::with_prefix("toctou_leaf_overwrite").unwrap();
    let dest = jail.path().join("dest");
    let canary = jail.path().join("canary");
    std::fs::create_dir(&dest).unwrap();
    std::fs::create_dir(&canary).unwrap();

    let victim = canary.join("victim.txt");
    std::fs::write(&victim, b"CANARY_INTACT").unwrap();

    // Case 1: pre-existing hard link (made read-only too) inside `dest` pointing to `victim`.
    let hl_path = dest.join("hl.txt");
    std::fs::hard_link(&victim, &hl_path).unwrap();
    let mut perms = std::fs::metadata(&hl_path).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&hl_path, perms).unwrap();

    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut w1 = ZipWriter::new(Cursor::new(Vec::new()));
    w1.start_file("hl.txt", options).unwrap();
    w1.write_all(b"EXTRACTED_HL").unwrap();
    let z1 = w1.finish().unwrap().into_inner();

    ZipArchive::new(Cursor::new(&z1))
        .unwrap()
        .extract(&dest)
        .unwrap();
    assert_eq!(std::fs::read(&victim).unwrap(), b"CANARY_INTACT");
    assert_eq!(std::fs::read(&hl_path).unwrap(), b"EXTRACTED_HL");

    // Same check for ZipStreamReader (consistent path).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut perms2 = std::fs::metadata(&victim).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        // Windows-only reset of the read-only attribute inside an isolated test TempDir.
        perms2.set_readonly(false);
        std::fs::set_permissions(&victim, perms2).unwrap();
    }
    std::fs::remove_file(&hl_path).unwrap();
    std::fs::hard_link(&victim, &hl_path).unwrap();
    ZipStreamReader::new(Cursor::new(&z1))
        .enforce_header_consistency(true)
        .extract(&dest)
        .unwrap();
    assert_eq!(std::fs::read(&victim).unwrap(), b"CANARY_INTACT");
    assert_eq!(std::fs::read(&hl_path).unwrap(), b"EXTRACTED_HL");

    // Case 2: pre-existing directory symlink/junction at leaf `dest/dirlink` -> `dest/inner`
    // (inside `dest`) cannot corrupt `inner/keep.txt` when extracting a regular file `dirlink`.
    let inner = dest.join("inner");
    std::fs::create_dir(&inner).unwrap();
    let inner_file = inner.join("keep.txt");
    std::fs::write(&inner_file, b"INNER_INTACT").unwrap();
    let dirlink = dest.join("dirlink");
    if create_dir_link(&inner, &dirlink).is_ok() {
        let mut w2 = ZipWriter::new(Cursor::new(Vec::new()));
        w2.start_file("dirlink", options).unwrap();
        w2.write_all(b"OVERWRITE_FILE").unwrap();
        let z2 = w2.finish().unwrap().into_inner();

        let _ = ZipArchive::new(Cursor::new(&z2)).unwrap().extract(&dest);
        assert_eq!(std::fs::read(&inner_file).unwrap(), b"INNER_INTACT");

        let _ = ZipStreamReader::new(Cursor::new(&z2))
            .enforce_header_consistency(true)
            .extract(&dest);
        assert_eq!(std::fs::read(&inner_file).unwrap(), b"INNER_INTACT");
    }

    // Case 3: duplicate regular entries where the first has read-only mode (0o444)
    // and the second overwrites it (`./dup_ro.txt`).
    let mut w3 = ZipWriter::new(Cursor::new(Vec::new()));
    w3.start_file("dup_ro.txt", options.unix_permissions(0o444))
        .unwrap();
    w3.write_all(b"OLD_RO").unwrap();
    w3.start_file("./dup_ro.txt", options.unix_permissions(0o644))
        .unwrap();
    w3.write_all(b"NEW_RW").unwrap();
    let z3 = w3.finish().unwrap().into_inner();
    ZipArchive::new(Cursor::new(&z3))
        .unwrap()
        .extract(&dest)
        .unwrap();
    assert_eq!(std::fs::read(dest.join("dup_ro.txt")).unwrap(), b"NEW_RW");
}

#[cfg(all(feature = "rayon", windows))]
#[test]
fn par_extract_windows_case_collision_is_deterministic_and_safe() {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let pairs = 30usize;
    for i in 0..pairs {
        writer.start_file(format!("case_{i}.txt"), options).unwrap();
        writer.write_all(format!("FIRST-{i}").as_bytes()).unwrap();
        writer.start_file(format!("CASE_{i}.TXT"), options).unwrap();
        writer.write_all(format!("SECOND-{i}").as_bytes()).unwrap();
    }
    let bytes = writer.finish().unwrap().into_inner();

    for round in 0..10 {
        let jail = TempDir::with_prefix("par_case_collision").unwrap();
        let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        archive
            .par_extract(jail.path())
            .unwrap_or_else(|e| panic!("round {round}: par_extract failed: {e:?}"));
        for i in 0..pairs {
            let content =
                std::fs::read_to_string(jail.path().join(format!("CASE_{i}.TXT"))).unwrap();
            assert_eq!(
                content,
                format!("SECOND-{i}"),
                "round {round}: later entry CASE_{i}.TXT lost race to case_{i}.txt"
            );
        }
    }
}

fn create_file_link(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link)
    }
}

#[test]
fn leaf_in_dest_symlink_replaced_without_overwriting_target_or_creating_ghost_dir() {
    let jail = TempDir::with_prefix("leaf_sym_poc").unwrap();
    let dest = jail.path().canonicalize().unwrap();

    // Probe whether file symlink creation is permitted on this host (Windows requires Developer Mode/Admin).
    let probe = dest.join("probe_link");
    if create_file_link(Path::new("probe_target"), &probe).is_err() {
        return;
    }
    let _ = std::fs::remove_file(&probe);

    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    // 1. Archive: symlink("x", "victim.txt"), file("victim.txt", b"KEEP"), file("./x", b"NEW"),
    //    plus dangling symlink("dangling", "ghost_dir/target"), file("./dangling", b"REPLACED"),
    //    plus dir("sub/"), symlink("dirlink", "sub"), file("./dirlink", b"OVERWROTE").
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    w.add_symlink("x", "victim.txt", options).unwrap();
    w.start_file("victim.txt", options).unwrap();
    w.write_all(b"KEEP").unwrap();
    w.start_file("./x", options).unwrap();
    w.write_all(b"NEW").unwrap();

    w.add_symlink("dangling", "ghost_dir/target", options)
        .unwrap();
    w.start_file("./dangling", options).unwrap();
    w.write_all(b"REPLACED").unwrap();

    w.add_directory("sub/", options).unwrap();
    w.add_symlink("dirlink", "sub", options).unwrap();
    w.start_file("./dirlink", options).unwrap();
    w.write_all(b"OVERWROTE").unwrap();

    let archive_bytes = w.finish().unwrap().into_inner();
    ZipArchive::new(Cursor::new(&archive_bytes))
        .unwrap()
        .extract(&dest)
        .unwrap();

    assert_eq!(
        std::fs::read(dest.join("victim.txt")).unwrap(),
        b"KEEP",
        "extracting regular file 'x' followed in-dest leaf symlink 'x' and overwrote victim.txt!"
    );
    assert!(
        !std::fs::symlink_metadata(dest.join("x"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "'x' remained a symlink instead of being replaced by the regular file"
    );
    assert_eq!(std::fs::read(dest.join("x")).unwrap(), b"NEW");
    assert!(
        !dest.join("ghost_dir").exists(),
        "resolving dangling leaf symlink spuriously created ghost_dir!"
    );
    assert_eq!(std::fs::read(dest.join("dangling")).unwrap(), b"REPLACED");
    assert!(dest.join("sub").is_dir());
    assert_eq!(std::fs::read(dest.join("dirlink")).unwrap(), b"OVERWROTE");

    // 2. ZipStreamReader::extract with pre-existing in-dest leaf symlink dest/sx -> svictim.txt.
    let stream_dest = jail.path().join("stream_dest");
    std::fs::create_dir(&stream_dest).unwrap();
    let stream_dest = stream_dest.canonicalize().unwrap();
    std::fs::write(stream_dest.join("svictim.txt"), b"KEEP_STREAM").unwrap();
    create_file_link(Path::new("svictim.txt"), &stream_dest.join("sx")).unwrap();

    let mut sw = ZipWriter::new(Cursor::new(Vec::new()));
    sw.start_file("sx", options).unwrap();
    sw.write_all(b"NEW_STREAM").unwrap();
    let sbytes = sw.finish().unwrap().into_inner();

    ZipStreamReader::new(Cursor::new(&sbytes))
        .enforce_header_consistency(true)
        .extract(&stream_dest)
        .unwrap();
    assert_eq!(
        std::fs::read(stream_dest.join("svictim.txt")).unwrap(),
        b"KEEP_STREAM"
    );
    assert_eq!(
        std::fs::read(stream_dest.join("sx")).unwrap(),
        b"NEW_STREAM"
    );
}

#[cfg(windows)]
#[test]
fn stream_extract_windows_case_collision_and_none_mode_clear_readonly() {
    use zip::System;

    // 1. Case collision in ZipStreamReader::extract (enforce=false): "A.TXT" (0o444) then "a.txt" (0o644)
    //    Notice `'a' > 'A'` in BTreeMap::rev(), so "a.txt" runs first and "A.TXT" runs AFTER unless case-folded!
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .system(System::Unix);
    let mut w1 = ZipWriter::new(Cursor::new(Vec::new()));
    w1.start_file("A.TXT", options.unix_permissions(0o444))
        .unwrap();
    w1.write_all(b"ro").unwrap();
    w1.start_file("a.txt", options.unix_permissions(0o644))
        .unwrap();
    w1.write_all(b"rw").unwrap();
    let z1 = w1.finish().unwrap().into_inner();

    let d1 = TempDir::with_prefix("stream_win_case").unwrap();
    ZipStreamReader::new(Cursor::new(&z1))
        .extract(d1.path())
        .unwrap();
    let ro1 = std::fs::metadata(d1.path().join("a.txt"))
        .unwrap()
        .permissions()
        .readonly();
    if ro1 {
        let mut p = std::fs::metadata(d1.path().join("a.txt"))
            .unwrap()
            .permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        let _ = std::fs::set_permissions(d1.path().join("a.txt"), p);
    }
    assert!(
        !ro1,
        "stale read-only entry for 'A.TXT' made overwriting 'a.txt' read-only!"
    );

    // 2. Duplicate normalized path (`./b.txt`) where second entry has unix_mode() == None (System::Dos, attr 0)
    let mut w2 = ZipWriter::new(Cursor::new(Vec::new()));
    w2.start_file("b.txt", options.unix_permissions(0o444))
        .unwrap();
    w2.write_all(b"ro").unwrap();
    w2.start_file(
        "./b.txt",
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .system(System::Unknown),
    )
    .unwrap();
    w2.write_all(b"rw").unwrap();
    let z2 = w2.finish().unwrap().into_inner();

    let d2 = TempDir::with_prefix("stream_win_none").unwrap();
    ZipStreamReader::new(Cursor::new(&z2))
        .extract(d2.path())
        .unwrap();
    let ro = std::fs::metadata(d2.path().join("b.txt"))
        .unwrap()
        .permissions()
        .readonly();
    if ro {
        let mut p = std::fs::metadata(d2.path().join("b.txt"))
            .unwrap()
            .permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        let _ = std::fs::set_permissions(d2.path().join("b.txt"), p);
    }
    assert!(
        !ro,
        "overwriting read-only entry with unix_mode()==None left file read-only!"
    );
}

#[cfg(windows)]
#[test]
fn windows_extracted_symlink_with_forward_slash_and_forward_dir_order() {
    let jail = TempDir::with_prefix("win_slash_sym").unwrap();
    let dest = jail.path().canonicalize().unwrap();
    let probe = dest.join("probe_link");
    if create_file_link(Path::new("probe_target"), &probe).is_err() {
        return;
    }
    let _ = std::fs::remove_file(&probe);

    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    w.start_file("sub/file.txt", options).unwrap();
    w.write_all(b"HELLO").unwrap();
    w.add_symlink("link", "sub/file.txt", options).unwrap();
    // Forward-ordered directory symlink: `dir_link -> later_dir` comes before `later_dir/`
    w.add_symlink("dir_link", "later_dir", options).unwrap();
    w.add_directory("later_dir/", options).unwrap();
    w.start_file("later_dir/inside.txt", options).unwrap();
    w.write_all(b"DIR_OK").unwrap();
    let bytes = w.finish().unwrap().into_inner();

    ZipArchive::new(Cursor::new(&bytes))
        .unwrap()
        .extract(&dest)
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(dest.join("link")).unwrap(),
        "HELLO",
        "Windows symlink with '/' target was unusable"
    );
    assert_eq!(
        std::fs::read_to_string(dest.join("dir_link").join("inside.txt")).unwrap(),
        "DIR_OK",
        "Windows forward-ordered directory symlink was not created as directory symlink"
    );
}

fn run_symlink_target_dir_race_check(symlink_action: zip::read::SymlinkExtractAction) {
    use zip::read::ExtractOptions;

    let jail = TempDir::with_prefix("sym_target_race").unwrap();
    let dest = jail.path().join("dest");
    let canary = jail.path().join("canary");
    std::fs::create_dir(&dest).unwrap();
    std::fs::create_dir(&canary).unwrap();
    let dest = dest.canonicalize().unwrap();
    let canary = canary.canonicalize().unwrap();
    std::fs::write(canary.join("secret.txt"), b"OUTSIDE").unwrap();

    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    // 1. Pre-existing intermediate junction `dest/sub -> canary` must reject `sub/secret.txt`.
    let sub_dir = dest.join("sub");
    if create_dir_link(&canary, &sub_dir).is_ok() {
        let mut w_static = ZipWriter::new(Cursor::new(Vec::new()));
        w_static
            .add_symlink("r0", "sub/secret.txt", options)
            .unwrap();
        let z_static = w_static.finish().unwrap().into_inner();
        assert!(
            ZipArchive::new(Cursor::new(&z_static))
                .unwrap()
                .extract_with_options(
                    &dest,
                    ExtractOptions::default().symlink_action(symlink_action)
                )
                .is_err(),
            "allowed symlink through pre-existing escaping intermediate link ({symlink_action:?})"
        );
        assert!(!dest.join("r0").exists());
        let _ = std::fs::remove_dir(&sub_dir);
        let _ = std::fs::remove_file(&sub_dir);
    }

    // 2. Concurrent target directory flip race during extraction.
    if !supports_dir_links(jail.path(), &canary) {
        return;
    }
    let mut w_race = ZipWriter::new(Cursor::new(Vec::new()));
    w_race.add_directory("sub/", options).unwrap();
    for i in 0..20 {
        w_race
            .add_symlink(format!("l{i}"), "sub/secret.txt", options)
            .unwrap();
    }
    let bytes = w_race.finish().unwrap().into_inner();

    let stop = Arc::new(AtomicBool::new(false));
    let flips = Arc::new(AtomicU64::new(0));
    let budget_ms = toctou_budget_ms(2_000);
    let racer = spawn_dir_racer(
        Arc::clone(&stop),
        Arc::clone(&flips),
        canary.clone(),
        sub_dir.clone(),
        false,
        true,
    );

    let (extracts, raced_extracts) = run_race_loop(budget_ms, &flips, || {
        for i in 0..20 {
            let _ = std::fs::remove_file(dest.join(format!("l{i}")));
        }
        let _ = std::fs::remove_dir(&sub_dir);
        let _ = std::fs::remove_file(&sub_dir);
        maybe_yield();
        let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let _ = archive.extract_with_options(
            &dest,
            ExtractOptions::default().symlink_action(symlink_action),
        );
        false
    });
    stop.store(true, Ordering::Relaxed);
    let _ = racer.join();
    let total_flips = flips.load(Ordering::Relaxed);
    assert!(total_flips > 0, "racer performed zero link flips");
    assert!(
        raced_extracts >= MIN_RACE_EXTRACTS,
        "only {raced_extracts} raced extractions (out of {extracts} total, {total_flips} flips)"
    );

    let _ = std::fs::remove_dir(&sub_dir);
    let _ = std::fs::remove_file(&sub_dir);
    let _ = std::fs::create_dir(&sub_dir);
    for i in 0..20 {
        let lp = dest.join(format!("l{i}"));
        if let Ok(target) = std::fs::read_link(&lp) {
            assert!(
                !target.is_absolute() && !target.to_string_lossy().contains("canary"),
                "created symlink {lp:?} escaped directly to {target:?}"
            );
        }
    }
}

#[test]
fn make_symlink_concurrent_target_dir_race_cannot_escape() {
    run_symlink_target_dir_race_check(zip::read::SymlinkExtractAction::ExtractInFolder);
}

#[test]
fn duplicate_unix_none_mode_and_extract_as_file_readonly_overwrite() {
    use zip::System;
    use zip::read::{ExtractOptions, SymlinkExtractAction};

    // 1. Archive with `a.txt` (0o444, System::Unix) followed by `./a.txt` (System::Unknown, unix_mode == None)
    let unix_opts = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .system(System::Unix);
    let unknown_opts = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .system(System::Unknown);
    let mut w1 = ZipWriter::new(Cursor::new(Vec::new()));
    w1.start_file("a.txt", unix_opts.unix_permissions(0o444))
        .unwrap();
    w1.write_all(b"ro").unwrap();
    w1.start_file("./a.txt", unknown_opts).unwrap();
    w1.write_all(b"rw").unwrap();
    let z1 = w1.finish().unwrap().into_inner();

    let d1 = TempDir::with_prefix("dup_none_mode").unwrap();
    ZipArchive::new(Cursor::new(&z1))
        .unwrap()
        .extract(d1.path())
        .unwrap();
    assert_eq!(std::fs::read(d1.path().join("a.txt")).unwrap(), b"rw");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            std::fs::metadata(d1.path().join("a.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o200,
            0,
            "prior entry's 0o444 leaked onto overwriting entry with unix_mode()==None"
        );
    }
    #[cfg(not(unix))]
    {
        let meta = std::fs::metadata(d1.path().join("a.txt")).unwrap();
        let ro = meta.permissions().readonly();
        if ro {
            let mut p = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            p.set_readonly(false);
            let _ = std::fs::set_permissions(d1.path().join("a.txt"), p);
        }
        assert!(
            !ro,
            "prior entry's read-only leaked onto overwriting entry with unix_mode()==None"
        );
    }

    // 2. ExtractAsFile symlink with 0o444 (System::Unix) followed by regular file ./link
    let mut w2 = ZipWriter::new(Cursor::new(Vec::new()));
    w2.add_symlink("link", "target", unix_opts.unix_permissions(0o444))
        .unwrap();
    w2.start_file("./link", unix_opts.unix_permissions(0o644))
        .unwrap();
    w2.write_all(b"OVERWROTE").unwrap();
    let z2 = w2.finish().unwrap().into_inner();

    let d2 = TempDir::with_prefix("extract_as_file_ro").unwrap();
    ZipArchive::new(Cursor::new(&z2))
        .unwrap()
        .extract_with_options(
            d2.path(),
            ExtractOptions::default().symlink_action(SymlinkExtractAction::ExtractAsFile),
        )
        .unwrap();
    assert_eq!(std::fs::read(d2.path().join("link")).unwrap(), b"OVERWROTE");
}

#[cfg(feature = "rayon")]
#[test]
fn par_extract_vs_extract_file_then_symlink_consistent() {
    let probe_dir = TempDir::with_prefix("probe_sym_cons").unwrap();
    let probe = probe_dir.path().join("probe");
    if create_file_link(Path::new("x"), &probe).is_err() {
        return;
    }

    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    w.start_file("foo", options).unwrap();
    w.write_all(b"original").unwrap();
    w.add_symlink("./foo", "bar", options).unwrap();
    let bytes = w.finish().unwrap().into_inner();

    let d_serial = TempDir::with_prefix("serial_fs").unwrap();
    let serial_res = ZipArchive::new(Cursor::new(&bytes))
        .unwrap()
        .extract(d_serial.path());

    let d_par = TempDir::with_prefix("par_fs").unwrap();
    let par_res = ZipArchive::new(Cursor::new(&bytes))
        .unwrap()
        .par_extract(d_par.path());

    assert_eq!(
        serial_res.is_ok(),
        par_res.is_ok(),
        "serial ({serial_res:?}) and parallel ({par_res:?}) diverged on file-then-symlink entry"
    );
}

#[cfg(feature = "rayon")]
#[test]
fn par_extract_vs_extract_mixed_types_consistent() {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    // 1. file("foo") then dir("./foo/")
    let mut w1 = ZipWriter::new(Cursor::new(Vec::new()));
    w1.start_file("foo", options).unwrap();
    w1.write_all(b"data").unwrap();
    w1.add_directory("./foo/", options).unwrap();
    let z1 = w1.finish().unwrap().into_inner();

    let d1s = TempDir::with_prefix("s_file_dir").unwrap();
    let r1s = ZipArchive::new(Cursor::new(&z1))
        .unwrap()
        .extract(d1s.path());
    let d1p = TempDir::with_prefix("p_file_dir").unwrap();
    let r1p = ZipArchive::new(Cursor::new(&z1))
        .unwrap()
        .par_extract(d1p.path());
    assert_eq!(
        r1s.is_ok(),
        r1p.is_ok(),
        "file-then-dir diverged: serial={r1s:?}, par={r1p:?}"
    );
    assert!(r1s.is_err(), "file-then-dir should reject conflict");

    // 2. dir("bar/") then file("./bar")
    let mut w2 = ZipWriter::new(Cursor::new(Vec::new()));
    w2.add_directory("bar/", options).unwrap();
    w2.start_file("./bar", options).unwrap();
    w2.write_all(b"data").unwrap();
    let z2 = w2.finish().unwrap().into_inner();

    let d2s = TempDir::with_prefix("s_dir_file").unwrap();
    let r2s = ZipArchive::new(Cursor::new(&z2))
        .unwrap()
        .extract(d2s.path());
    let d2p = TempDir::with_prefix("p_dir_file").unwrap();
    let r2p = ZipArchive::new(Cursor::new(&z2))
        .unwrap()
        .par_extract(d2p.path());
    assert_eq!(
        r2s.is_ok(),
        r2p.is_ok(),
        "dir-then-file diverged: serial={r2s:?}, par={r2p:?}"
    );
    assert!(r2s.is_err(), "dir-then-file should reject conflict");

    // 3. dir("baz/") then symlink("./baz", "target")
    let probe_dir = TempDir::with_prefix("probe_dir_sym").unwrap();
    if create_file_link(Path::new("x"), &probe_dir.path().join("probe")).is_ok() {
        let mut w3 = ZipWriter::new(Cursor::new(Vec::new()));
        w3.add_directory("baz/", options).unwrap();
        w3.add_symlink("./baz", "target", options).unwrap();
        let z3 = w3.finish().unwrap().into_inner();

        let d3s = TempDir::with_prefix("s_dir_sym").unwrap();
        let r3s = ZipArchive::new(Cursor::new(&z3))
            .unwrap()
            .extract(d3s.path());
        let d3p = TempDir::with_prefix("p_dir_sym").unwrap();
        let r3p = ZipArchive::new(Cursor::new(&z3))
            .unwrap()
            .par_extract(d3p.path());
        assert_eq!(
            r3s.is_ok(),
            r3p.is_ok(),
            "dir-then-symlink diverged: serial={r3s:?}, par={r3p:?}"
        );
        assert!(r3s.is_err(), "dir-then-symlink should reject conflict");
    }
}

#[test]
fn extract_recursive_in_folder_target_dir_race_and_outside_escape() {
    use zip::read::{ExtractOptions, SymlinkExtractAction};

    let jail = TempDir::with_prefix("rec_in_folder_race").unwrap();
    let dest = jail.path().join("dest");
    let canary = jail.path().join("canary");
    std::fs::create_dir(&dest).unwrap();
    std::fs::create_dir(&canary).unwrap();
    let dest = dest.canonicalize().unwrap();
    let canary = canary.canonicalize().unwrap();
    std::fs::write(canary.join("secret.txt"), b"OUTSIDE").unwrap();

    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    // 1. Static escaping relative target under ExtractRecursiveInFolder must be rejected.
    let mut w_esc = ZipWriter::new(Cursor::new(Vec::new()));
    w_esc
        .add_symlink("esc", "../canary/secret.txt", options)
        .unwrap();
    let z_esc = w_esc.finish().unwrap().into_inner();
    assert!(
        ZipArchive::new(Cursor::new(&z_esc))
            .unwrap()
            .extract_with_options(
                &dest,
                ExtractOptions::default()
                    .symlink_action(SymlinkExtractAction::ExtractRecursiveInFolder),
            )
            .is_err()
    );
    assert!(!dest.join("esc").exists());

    // 2. Static intermediate junction `dest/sub -> canary` under ExtractRecursiveInFolder must be rejected.
    let sub_dir = dest.join("sub");
    if create_dir_link(&canary, &sub_dir).is_ok() {
        let mut w_static = ZipWriter::new(Cursor::new(Vec::new()));
        w_static
            .add_symlink("r0", "sub/secret.txt", options)
            .unwrap();
        let z_static = w_static.finish().unwrap().into_inner();
        assert!(
            ZipArchive::new(Cursor::new(&z_static))
                .unwrap()
                .extract_with_options(
                    &dest,
                    ExtractOptions::default()
                        .symlink_action(SymlinkExtractAction::ExtractRecursiveInFolder),
                )
                .is_err(),
            "ExtractRecursiveInFolder allowed symlink through pre-existing escaping intermediate link"
        );
        assert!(!dest.join("r0").exists());
        let _ = std::fs::remove_dir(&sub_dir);
        let _ = std::fs::remove_file(&sub_dir);
    }

    // 3. Concurrent intermediate target dir race under ExtractRecursiveInFolder:
    //    `r{i}` points to `sub/secret.txt`; pin prevents `sub` swap while `r{i}` is created,
    //    and after stopping the racer we confirm no created link escapes outside `dest`.
    if !supports_dir_links(jail.path(), &canary) {
        return;
    }
    let mut w_race = ZipWriter::new(Cursor::new(Vec::new()));
    w_race.add_directory("sub/", options).unwrap();
    for i in 0..20 {
        w_race
            .add_symlink(format!("r{i}"), "sub/secret.txt", options)
            .unwrap();
    }
    let z_race = w_race.finish().unwrap().into_inner();

    let stop = Arc::new(AtomicBool::new(false));
    let flips = Arc::new(AtomicU64::new(0));
    let budget_ms = toctou_budget_ms(2_000);
    let racer = spawn_dir_racer(
        Arc::clone(&stop),
        Arc::clone(&flips),
        canary.clone(),
        sub_dir.clone(),
        false,
        true,
    );

    let (extracts, raced_extracts) = run_race_loop(budget_ms, &flips, || {
        for i in 0..20 {
            let _ = std::fs::remove_file(dest.join(format!("r{i}")));
        }
        let _ = std::fs::remove_dir(&sub_dir);
        let _ = std::fs::remove_file(&sub_dir);
        maybe_yield();
        let mut archive = ZipArchive::new(Cursor::new(&z_race)).unwrap();
        let _ = archive.extract_with_options(
            &dest,
            ExtractOptions::default()
                .symlink_action(SymlinkExtractAction::ExtractRecursiveInFolder),
        );
        false
    });
    stop.store(true, Ordering::Relaxed);
    let _ = racer.join();
    let total_flips = flips.load(Ordering::Relaxed);
    assert!(total_flips > 0, "racer performed zero link flips");
    assert!(
        raced_extracts >= MIN_RACE_EXTRACTS,
        "only {raced_extracts} raced extractions (out of {extracts} total, {total_flips} flips)"
    );

    // Ensure `sub` is restored as a normal directory before inspecting the link targets.
    let _ = std::fs::remove_dir(&sub_dir);
    let _ = std::fs::remove_file(&sub_dir);
    let _ = std::fs::create_dir(&sub_dir);
    for i in 0..20 {
        let rp = dest.join(format!("r{i}"));
        if let Ok(target) = std::fs::read_link(&rp) {
            assert!(
                !target.is_absolute() && !target.to_string_lossy().contains("canary"),
                "created symlink {rp:?} escaped directly to {target:?}"
            );
        }
    }
}

#[test]
fn destination_permissions_are_never_altered_across_extractors() {
    use zip::read::root_dir_common_filter;

    #[cfg(unix)]
    let (target_dir_mode, expected_preserved_mode) = (0o777u32, 0o700u32);
    #[cfg(not(unix))]
    let target_dir_mode = 0o444u32; // triggers `readonly` on Windows if `path == dest` is not skipped

    let check_dest_preserved = |dest: &Path| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let got = std::fs::metadata(dest).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                got, expected_preserved_mode,
                "destination {dest:?} mode mutated to {got:#o}"
            );
        }
        #[cfg(not(unix))]
        {
            assert!(
                !std::fs::metadata(dest).unwrap().permissions().readonly(),
                "destination {dest:?} became readonly"
            );
        }
    };

    let prep_dest = |parent: &Path, name: &str| {
        let p = parent.join(name);
        std::fs::create_dir(&p).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(expected_preserved_mode))
                .unwrap();
        }
        p
    };

    let build_dot_archive = |entry_name: &str| {
        let opts = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(target_dir_mode);
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        w.add_directory(entry_name, opts).unwrap();
        w.start_file(
            "child.txt",
            SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .unix_permissions(0o644),
        )
        .unwrap();
        w.write_all(b"ok").unwrap();
        w.finish().unwrap().into_inner()
    };

    let root = TempDir::with_prefix("toctou_dest_perm").unwrap();

    // 1. `./`, nested/redundant dot spellings, `../` collapsing to `dest`, and `/`
    //    across serial extract, stream extract (non-strict & strict), par_extract,
    //    and non-existent destinations.
    for (idx, entry_name) in [
        "./",
        "././",
        ".//",
        r".\",
        "safe/../",
        "safe/.././",
        "a/b/../../",
        "/",
        "/./",
    ]
    .iter()
    .enumerate()
    {
        let zip_bytes = build_dot_archive(entry_name);

        let d_serial = prep_dest(root.path(), &format!("serial_{idx}"));
        ZipArchive::new(Cursor::new(&zip_bytes))
            .unwrap()
            .extract(&d_serial)
            .unwrap();
        check_dest_preserved(&d_serial);
        assert_eq!(std::fs::read(d_serial.join("child.txt")).unwrap(), b"ok");

        for enforce in [false, true] {
            let d_stream = prep_dest(root.path(), &format!("stream_{idx}_{enforce}"));
            let reader =
                ZipStreamReader::new(Cursor::new(&zip_bytes)).enforce_header_consistency(enforce);
            reader.extract(&d_stream).unwrap();
            check_dest_preserved(&d_stream);
            assert_eq!(std::fs::read(d_stream.join("child.txt")).unwrap(), b"ok");
        }

        let d_new = root.path().join(format!("fresh_{idx}"));
        ZipArchive::new(Cursor::new(&zip_bytes))
            .unwrap()
            .extract(&d_new)
            .unwrap();
        assert_eq!(std::fs::read(d_new.join("child.txt")).unwrap(), b"ok");

        #[cfg(feature = "rayon")]
        {
            let d_par = prep_dest(root.path(), &format!("par_{idx}"));
            ZipArchive::new(Cursor::new(&zip_bytes))
                .unwrap()
                .par_extract(&d_par)
                .unwrap();
            check_dest_preserved(&d_par);
            assert_eq!(std::fs::read(d_par.join("child.txt")).unwrap(), b"ok");

            let d_par_new = root.path().join(format!("fresh_par_{idx}"));
            ZipArchive::new(Cursor::new(&zip_bytes))
                .unwrap()
                .par_extract(&d_par_new)
                .unwrap();
            assert_eq!(std::fs::read(d_par_new.join("child.txt")).unwrap(), b"ok");
        }
    }

    // 2. Unwrapped single root dir (`package/` resolving to `dest`):
    let mut w_unwrap = ZipWriter::new(Cursor::new(Vec::new()));
    w_unwrap
        .add_directory(
            "package/",
            SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .unix_permissions(target_dir_mode),
        )
        .unwrap();
    w_unwrap
        .start_file(
            "package/inner.txt",
            SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .unix_permissions(0o644),
        )
        .unwrap();
    w_unwrap.write_all(b"unwrapped").unwrap();
    let z_unwrap = w_unwrap.finish().unwrap().into_inner();

    let d_unwrap = prep_dest(root.path(), "unwrap_serial");
    ZipArchive::new(Cursor::new(&z_unwrap))
        .unwrap()
        .extract_unwrapped_root_dir(&d_unwrap, root_dir_common_filter)
        .unwrap();
    check_dest_preserved(&d_unwrap);
    assert_eq!(
        std::fs::read(d_unwrap.join("inner.txt")).unwrap(),
        b"unwrapped"
    );

    #[cfg(feature = "rayon")]
    {
        use zip::read::ExtractOptions;
        let d_par_unwrap = prep_dest(root.path(), "unwrap_par");
        ZipArchive::new(Cursor::new(&z_unwrap))
            .unwrap()
            .par_extract_with_options(
                &d_par_unwrap,
                ExtractOptions::default().with_root_dir_filter(root_dir_common_filter),
            )
            .unwrap();
        check_dest_preserved(&d_par_unwrap);
        assert_eq!(
            std::fs::read(d_par_unwrap.join("inner.txt")).unwrap(),
            b"unwrapped"
        );
    }

    // 3. Regular-file entry (`"."`, no trailing slash so `is_dir()` is false) resolving to `dest`
    //    must be rejected and never clobber the destination directory or its permissions.
    let mut w_file_dot = ZipWriter::new(Cursor::new(Vec::new()));
    w_file_dot
        .start_file(
            ".",
            SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .unix_permissions(target_dir_mode),
        )
        .unwrap();
    w_file_dot.write_all(b"evil").unwrap();
    let z_file_dot = w_file_dot.finish().unwrap().into_inner();

    let d_file_dot = prep_dest(root.path(), "file_dot");
    assert!(
        ZipArchive::new(Cursor::new(&z_file_dot))
            .unwrap()
            .extract(&d_file_dot)
            .is_err()
    );
    check_dest_preserved(&d_file_dot);
    assert!(d_file_dot.is_dir());

    // 4. Symlink inside `dest` pointing to `.` (`dest`) followed by a directory entry
    //    targeting that symlink name must not alter `dest`'s permissions either.
    let d_selflink = prep_dest(root.path(), "selflink_dot");
    let link_path = d_selflink.join("selflink");
    if create_dir_link(&d_selflink, &link_path).is_ok() {
        let mut w_self = ZipWriter::new(Cursor::new(Vec::new()));
        w_self
            .add_directory(
                "selflink/",
                SimpleFileOptions::default()
                    .compression_method(CompressionMethod::Stored)
                    .unix_permissions(target_dir_mode),
            )
            .unwrap();
        let z_self = w_self.finish().unwrap().into_inner();
        let _ = ZipArchive::new(Cursor::new(&z_self))
            .unwrap()
            .extract(&d_selflink);
        check_dest_preserved(&d_selflink);
    }
}
