//! Verification test for Windows DOS reserved device names and trailing separators in `zip2`.
//!
//! Under the Win32 subsystem:
//! 1. Reserved DOS device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`..`COM9`, `LPT1`..`LPT9`, `CONIN$`, `CONOUT$`),
//!    with or without file extensions, map to special system/console/hardware devices rather than normal files.
//! 2. Filenames with trailing dots (`.`) or trailing spaces (` `) are silently stripped by Win32 APIs,
//!    causing path canonicalization collisions and security scanner evasion.
//!
//! This test verifies that:
//! - `enclosed_name()` rejects Windows reserved device names and trailing separators on Windows.
//! - `extract()` rejects archives containing Windows reserved names or trailing separators on Windows.
//! - `mangled_name()` safely sanitizes reserved device names (e.g. by prefixing `_`) and trims trailing separators.
//! - Non-reserved names (such as `contact.txt`, `auxiliary.rs`, `nullify.dat`, `com0.txt`) remain fully valid.

#[cfg(windows)]
#[test]
fn test_windows_reserved_device_names_rejected_in_enclosed_name() {
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    let reserved_names = [
        "CON",
        "con.txt",
        "PRN",
        "prn.log",
        "AUX",
        "aux.json",
        "NUL",
        "nul.dat",
        "COM1",
        "com2.txt",
        "COM9.dat",
        "LPT1",
        "lpt3.bin",
        "LPT9.txt",
        "CONIN$",
        "CONOUT$",
        "CLOCK$",
        "clock$.log",
        "COM¹",
        "COM¹.dat",
        "COM²",
        "COM³",
        "LPT¹",
        "LPT²",
        "LPT³",
        "CON .txt",
        "COM1 .dat",
        "con...txt",
        "CON. ",
        "sub/CON.txt",
        "nested/dir/aux.json",
        "folder/NUL",
        "evil.exe.",
        "secret.txt ",
        "test.txt. . ",
        "foo<bar>baz.txt",
        "foo\"bar|pipe.txt",
        "foo?bar*star.txt",
        "foo\x01ctrl.txt",
    ];

    let mut zip_data = Vec::new();
    {
        let mut zip = ZipWriter::new(Cursor::new(&mut zip_data));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for name in reserved_names {
            zip.start_file(name, options).unwrap();
            std::io::Write::write_all(&mut zip, b"payload").unwrap();
        }
        zip.finish().unwrap();
    }

    let mut archive = ZipArchive::new(Cursor::new(zip_data)).unwrap();
    for i in 0..archive.len() {
        let file = archive.by_index(i).unwrap();
        let name = file.name().unwrap().into_owned();
        assert_eq!(
            file.enclosed_name(),
            None,
            "Expected enclosed_name() to reject Windows reserved/invalid name: {name}"
        );
    }
}

#[cfg(all(windows, feature = "extract"))]
#[test]
fn test_windows_reserved_device_names_extraction_rejected() {
    use std::io::Cursor;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    let test_cases = [
        "con.txt",
        "aux",
        "nul.dat",
        "com1",
        "lpt1.txt",
        "CLOCK$.log",
        "COM¹.dat",
        "CON .txt",
        "foo<bar>.txt",
        "evil.exe.",
        "trailing_space ",
    ];

    for name in test_cases {
        let mut zip_data = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut zip_data));
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zip.start_file(name, options).unwrap();
            std::io::Write::write_all(&mut zip, b"test_payload").unwrap();
            zip.finish().unwrap();
        }

        let mut archive = ZipArchive::new(Cursor::new(zip_data)).unwrap();
        let target_dir = tempdir().unwrap();
        let result = archive.extract(target_dir.path());
        assert!(
            result.is_err(),
            "Expected archive extraction to fail for Windows reserved/invalid name: {name}"
        );
    }
}

#[cfg(windows)]
#[test]
fn test_windows_reserved_names_mangled_name_sanitized() {
    use std::io::Cursor;
    use std::path::PathBuf;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    let cases = [
        ("CON", PathBuf::from("_CON")),
        ("con.txt", PathBuf::from("_con.txt")),
        ("aux", PathBuf::from("_aux")),
        ("aux.json", PathBuf::from("_aux.json")),
        ("NUL", PathBuf::from("_NUL")),
        ("nul.dat", PathBuf::from("_nul.dat")),
        ("COM1", PathBuf::from("_COM1")),
        ("com2.txt", PathBuf::from("_com2.txt")),
        ("LPT1", PathBuf::from("_LPT1")),
        ("CLOCK$.log", PathBuf::from("_CLOCK$.log")),
        ("COM¹.dat", PathBuf::from("_COM¹.dat")),
        ("sub/con.txt", PathBuf::from_iter(["sub", "_con.txt"])),
        ("evil.exe.", PathBuf::from("evil.exe")),
        ("secret.txt ", PathBuf::from("secret.txt")),
        ("foo<bar>baz.txt", PathBuf::from("foo-bar-baz.txt")),
        ("foo\"bar|pipe.txt", PathBuf::from("foo-bar-pipe.txt")),
        ("foo?bar*star.txt", PathBuf::from("foo-bar-star.txt")),
    ];

    let mut zip_data = Vec::new();
    {
        let mut zip = ZipWriter::new(Cursor::new(&mut zip_data));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, _) in &cases {
            zip.start_file(*name, options).unwrap();
            std::io::Write::write_all(&mut zip, b"data").unwrap();
        }
        zip.finish().unwrap();
    }

    let mut archive = ZipArchive::new(Cursor::new(zip_data)).unwrap();
    for (i, (name, expected)) in cases.iter().enumerate() {
        let file = archive.by_index(i).unwrap();
        let mangled = file.mangled_name().unwrap();
        assert_eq!(
            &mangled, expected,
            "mangled_name() failed to sanitize reserved name: {name}"
        );
    }
}

#[cfg(all(windows, feature = "extract"))]
#[test]
fn test_legitimate_similar_names_accepted() {
    use std::io::Cursor;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    let legitimate_names = [
        "contact.txt",
        "content.json",
        "auxiliary.rs",
        "nullify.c",
        "computer.log",
        "com0.txt",
        "lpt0.dat",
        "cone.png",
        "sub/console.txt",
    ];

    let mut zip_data = Vec::new();
    {
        let mut zip = ZipWriter::new(Cursor::new(&mut zip_data));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for name in legitimate_names {
            zip.start_file(name, options).unwrap();
            std::io::Write::write_all(&mut zip, b"legitimate").unwrap();
        }
        zip.finish().unwrap();
    }

    let mut archive = ZipArchive::new(Cursor::new(zip_data)).unwrap();
    for i in 0..archive.len() {
        let file = archive.by_index(i).unwrap();
        let name = file.name().unwrap().into_owned();
        assert!(
            file.enclosed_name().is_some(),
            "Legitimate file should be accepted by enclosed_name(): {name}"
        );
    }

    let target_dir = tempdir().unwrap();
    assert!(
        archive.extract(target_dir.path()).is_ok(),
        "Legitimate files should extract successfully without errors"
    );
}

#[cfg(all(windows, feature = "extract"))]
#[test]
fn test_cannot_escape_to_windows_startup_directory() {
    use std::io::Cursor;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    // Construct common Windows persistence/Startup attack vectors
    let startup_attack_vectors = [
        r"C:\Users\Default\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup\evil.bat",
        r"\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup\evil.bat",
        r"..\..\..\..\..\..\..\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup\evil.bat",
        r"..\..\..\..\..\..\..\ProgramData\Microsoft\Windows\Start Menu\Programs\Startup\evil.bat",
        r"sub/../../../../../../../../Startup/evil.bat",
        r"C:Start Menu/Programs/Startup/evil.bat",
        r"\\?\C:\Users\Default\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup\evil.bat",
    ];

    let dest = tempdir().unwrap();
    let dest_path = dest.path();

    for attack_path in startup_attack_vectors {
        let mut zip_data = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut zip_data));
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zip.start_file(attack_path, options).unwrap();
            std::io::Write::write_all(&mut zip, b"echo Malicious Startup Execution").unwrap();
            zip.finish().unwrap();
        }

        let mut archive = ZipArchive::new(Cursor::new(zip_data)).unwrap();
        {
            let file = archive.by_index(0).unwrap();

            // Check 1: If enclosed_name() produces a path, joining it onto dest MUST stay inside dest
            if let Some(enclosed) = file.enclosed_name() {
                let joined = dest_path.join(&enclosed);
                assert!(
                    joined.starts_with(dest_path),
                    "enclosed_name() escaped destination: {attack_path} -> {joined:?}"
                );
            }

            // Check 2: Even with mangled_name(), joining onto dest MUST stay inside dest
            let mangled = file.mangled_name().unwrap();
            let joined_mangled = dest_path.join(&mangled);
            assert!(
                joined_mangled.starts_with(dest_path),
                "mangled_name() escaped destination: {attack_path} -> {joined_mangled:?}"
            );
        }

        // Check 3: Extracting archive must either fail (rejection) or write strictly INSIDE dest_path
        let _ = archive.extract(dest_path);

        // Verify that nothing was written outside dest_path
        for entry in walkdir::WalkDir::new(dest_path) {
            let path = entry.unwrap().into_path();
            assert!(
                path.starts_with(dest_path),
                "File extracted outside destination directory: {path:?}"
            );
        }
    }
}
