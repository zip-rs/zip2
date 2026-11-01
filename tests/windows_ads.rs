//! Windows NTFS Alternate Data Stream (ADS) verification test for `zip2`.
//! Verifies that `zip2` currently permits creating Alternate Data Streams (e.g. `Zone.Identifier`)
//! because colons in component names are treated as valid path characters instead of being rejected.

#[cfg(windows)]
#[test]
fn test_windows_ads_stream_creation() -> Result<(), Box<dyn std::error::Error>> {
    use std::fs::File;
    use std::io::Write;
    use std::process::Command;
    use tempfile::tempdir;
    use zip::ZipWriter;
    use zip::write::{ExtendedFileOptions, FileOptions};

    // 1. Temporary extraction target
    let target_dir = tempdir()?;
    let target_path = target_dir.path();

    // 2. Build ZIP containing a base file and an ADS stream
    let mut zip_data = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut zip_data));
        let options: FileOptions<'_, '_, ExtendedFileOptions> = FileOptions::default()
            .unix_permissions(0o644)
            .last_modified_time(
                zip::DateTime::from_date_and_time(2024, 1, 1, 0, 0, 0).expect("invalid datetime"),
            );

        // Primary file
        zip.start_file("target_app.exe", options.clone())?;
        zip.write_all(b"PE_EXECUTABLE_DUMMY_PAYLOAD")?;

        // Alternate Data Stream targeting Mark-of-the-Web (Zone.Identifier)
        zip.start_file("target_app.exe:Zone.Identifier", options)?;
        zip.write_all(b"[ZoneTransfer]\r\nZoneId=0\r\n")?;

        zip.finish()?;
    }

    // 3. Write archive to disk
    let zip_path = target_path.join("ads_poc.zip");
    {
        let mut f = File::create(&zip_path)?;
        f.write_all(&zip_data)?;
    }

    // 4. Attempt to extract archive using zip::ZipArchive
    let file = File::open(&zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let result = archive.extract(target_path);

    // Extraction MUST fail because component contains an invalid character / ADS specifier (':')
    assert!(
        result.is_err(),
        "Expected extraction to fail for ADS entry, but it succeeded"
    );

    // 5. Query the Alternate Data Stream using PowerShell to ensure it was NOT created
    let ads_stream_name = "Zone.Identifier";
    let base_file_path = target_path.join("target_app.exe");

    if base_file_path.exists() {
        let read_cmd = format!(
            "Get-Content -Path '{}' -Stream '{}' -Raw",
            base_file_path.display(),
            ads_stream_name
        );

        let output = Command::new("powershell")
            .args(["-NoProfile", "-Command", &read_cmd])
            .output()?;

        assert!(
            !output.status.success(),
            "Alternate Data Stream was created despite extraction rejection"
        );
    }

    Ok(())
}

use zip::ZipExtract;
