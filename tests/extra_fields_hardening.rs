//! Regression tests for ZIP64, UnicodePath (0x7075), data descriptor, and AES extra-field edge cases.

use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

use zip::CompressionMethod;
use zip::extra_fields::ExtraField;
use zip::read::{Config, ZipArchive, ZipFile, ZipFileEntry, ZipReadOptions};
use zip::result::{ZipError, ZipResult};
use zip::unstable::format::data_descriptor::ZipDataDescriptor;
use zip::unstable::read::ZipIterable;
use zip::unstable::stream::{ZipStreamReader, ZipStreamVisitor};
use zip::unstable::write::FileOptionsExt;
use zip::write::{FullFileOptions, SimpleFileOptions, ZipWriter};

/// Offset past the 32-bit boundary so entries require a ZIP64 relative header offset without
/// allocating 4 GiB of memory.
const BASE: u64 = 1 << 32;

fn as_usize_clamped(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// Sparse in-memory `Read + Write + Seek` whose first `BASE` bytes are virtual zeros.
#[derive(Default)]
struct Shifted {
    data: Vec<u8>,
    pos: u64,
}

impl Write for Shifted {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Some(at) = self.pos.checked_sub(BASE) else {
            return Err(io::Error::other(format!(
                "write into unstored region below BASE at offset {}",
                self.pos
            )));
        };
        let at =
            usize::try_from(at).map_err(|_| io::Error::other("write offset overflows usize"))?;
        let end = at
            .checked_add(buf.len())
            .ok_or_else(|| io::Error::other("write end overflows usize"))?;
        if self.data.len() < end {
            self.data.resize(end, 0);
        }
        self.data[at..end].copy_from_slice(buf);
        self.pos = self
            .pos
            .checked_add(buf.len() as u64)
            .ok_or_else(|| io::Error::other("position overflow"))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for Shifted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = if self.pos < BASE {
            let n = buf.len().min(as_usize_clamped(BASE - self.pos));
            buf[..n].fill(0);
            n
        } else {
            let at = as_usize_clamped(self.pos - BASE).min(self.data.len());
            let n = buf.len().min(self.data.len() - at);
            buf[..n].copy_from_slice(&self.data[at..at + n]);
            n
        };
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Shifted {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::End(delta) => (BASE + self.data.len() as u64).checked_add_signed(delta),
            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
        };
        self.pos = target.ok_or_else(|| io::Error::other("seek before start"))?;
        Ok(self.pos)
    }
}

fn make_upath_payload(crc: u32, content: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(5 + content.len());
    data.push(1u8); // version
    data.extend_from_slice(&crc.to_le_bytes());
    data.extend_from_slice(content);
    data
}

struct NoopVisitor;

impl ZipStreamVisitor for NoopVisitor {
    fn visit_file<R: Read>(&mut self, file: &mut ZipFile<'_, R>) -> ZipResult<()> {
        io::copy(file, &mut io::sink())?;
        Ok(())
    }

    fn visit_additional_metadata(&mut self, _metadata: &ZipFileEntry<'_>) -> ZipResult<()> {
        Ok(())
    }
}

struct RawSingleEntrySpec<'a> {
    local_name: &'a [u8],
    local_extra: &'a [u8],
    local_comp_size: u32,
    local_uncomp_size: u32,
    cd_name: &'a [u8],
    cd_extra: &'a [u8],
    cd_comp_size: u32,
    cd_uncomp_size: u32,
    cd_offset: u32,
    payload: &'a [u8],
    extra_local_headers: &'a [Vec<u8>],
}

/// Build a minimal stored single-entry ZIP archive directly from raw header fields so adversarial
/// extra-field combinations can be constructed deterministically.
fn build_raw_single_entry_zip(spec: &RawSingleEntrySpec<'_>) -> Vec<u8> {
    let RawSingleEntrySpec {
        local_name,
        local_extra,
        local_comp_size,
        local_uncomp_size,
        cd_name,
        cd_extra,
        cd_comp_size,
        cd_uncomp_size,
        cd_offset,
        payload,
        extra_local_headers,
    } = *spec;
    let crc = crc32fast::hash(payload);
    let mut out = Vec::new();

    // Primary local file header at offset 0
    out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    out.extend_from_slice(&45u16.to_le_bytes()); // version needed
    out.extend_from_slice(&0u16.to_le_bytes()); // flags
    out.extend_from_slice(&0u16.to_le_bytes()); // stored
    out.extend_from_slice(&0u16.to_le_bytes()); // mod time
    out.extend_from_slice(&0x0021u16.to_le_bytes()); // mod date
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&local_comp_size.to_le_bytes());
    out.extend_from_slice(&local_uncomp_size.to_le_bytes());
    out.extend_from_slice(&(local_name.len() as u16).to_le_bytes());
    out.extend_from_slice(&(local_extra.len() as u16).to_le_bytes());
    out.extend_from_slice(local_name);
    out.extend_from_slice(local_extra);
    out.extend_from_slice(payload);

    for extra_lfh in extra_local_headers {
        out.extend_from_slice(extra_lfh);
    }

    let cd_start = out.len() as u32;
    out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
    out.extend_from_slice(&45u16.to_le_bytes()); // version made by
    out.extend_from_slice(&45u16.to_le_bytes()); // version needed
    out.extend_from_slice(&0u16.to_le_bytes()); // flags
    out.extend_from_slice(&0u16.to_le_bytes()); // stored
    out.extend_from_slice(&0u16.to_le_bytes()); // mod time
    out.extend_from_slice(&0x0021u16.to_le_bytes()); // mod date
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&cd_comp_size.to_le_bytes());
    out.extend_from_slice(&cd_uncomp_size.to_le_bytes());
    out.extend_from_slice(&(cd_name.len() as u16).to_le_bytes());
    out.extend_from_slice(&(cd_extra.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment len
    out.extend_from_slice(&0u16.to_le_bytes()); // disk number
    out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
    out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(cd_name);
    out.extend_from_slice(cd_extra);
    let cd_size = (out.len() as u32) - cd_start;

    // EOCD
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn build_local_entry_bytes(name: &[u8], payload: &[u8]) -> Vec<u8> {
    let crc = crc32fast::hash(payload);
    let mut out = Vec::new();
    out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    out.extend_from_slice(&45u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0x0021u16.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(payload);
    out
}

fn count_local_extra_id<R: Read + Seek>(
    reader: &mut R,
    header_start: u64,
    target_id: u16,
) -> usize {
    reader.seek(SeekFrom::Start(header_start)).unwrap();
    let mut fixed = [0u8; 30];
    reader.read_exact(&mut fixed).unwrap();
    assert_eq!(&fixed[0..4], &0x0403_4b50u32.to_le_bytes());
    let name_len = u16::from_le_bytes([fixed[26], fixed[27]]) as usize;
    let extra_len = u16::from_le_bytes([fixed[28], fixed[29]]) as usize;
    reader.seek(SeekFrom::Current(name_len as i64)).unwrap();
    let mut extra = vec![0u8; extra_len];
    reader.read_exact(&mut extra).unwrap();
    let mut pos = 0;
    let mut count = 0;
    while pos + 4 <= extra.len() {
        let id = u16::from_le_bytes([extra[pos], extra[pos + 1]]);
        let sz = u16::from_le_bytes([extra[pos + 2], extra[pos + 3]]) as usize;
        if id == target_id {
            count += 1;
        }
        pos = pos.saturating_add(4 + sz);
    }
    count
}

// ============================================================================
// F1: Shadowed / duplicate UnicodePath (0x7075) bypasses enforce_header_consistency
// ============================================================================

#[test]
fn f1_enforce_header_consistency_rejects_shadowed_malicious_unicode_path() {
    let header_name = b"safe.txt";
    let malicious_name = "sub/evil\u{202E}txt.exe".as_bytes();

    // First 0x7075 matches header_name's CRC and carries a BiDi-override + path-depth-mismatch name.
    // Second 0x7075 has a mismatched CRC and benign content "safe.txt", shadowing the first in `get_upath`.
    let mut options = FullFileOptions::default().compression_method(CompressionMethod::Stored);
    options
        .add_extra_field(
            0x7075,
            make_upath_payload(crc32fast::hash(header_name), malicious_name),
            false,
        )
        .unwrap();
    options
        .add_extra_field(0x7075, make_upath_payload(0xDEAD_BEEF, header_name), false)
        .unwrap();

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("safe.txt", options).unwrap();
    writer.write_all(b"payload").unwrap();
    let bytes = writer.finish().unwrap().into_inner();

    // ZipArchive with enforce_header_consistency(true) MUST reject this.
    let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let res =
        archive.by_index_with_options(0, ZipReadOptions::new().enforce_header_consistency(true));
    assert!(
        matches!(res, Err(ZipError::InvalidArchive(_))),
        "ZipArchive must reject shadowed malicious UnicodePath under enforce_header_consistency(true)"
    );

    // ZipIterable with enforce_header_consistency(true) MUST reject this.
    let mut iterable = ZipIterable::try_new(Cursor::new(&bytes), Config::default())
        .unwrap()
        .enforce_header_consistency(true);
    let iter_res: ZipResult<Vec<_>> = iterable.files().unwrap().collect();
    assert!(
        matches!(iter_res, Err(ZipError::InvalidArchive(_))),
        "ZipIterable must reject shadowed malicious UnicodePath under enforce_header_consistency(true)"
    );

    // ZipStreamReader with enforce_header_consistency(true) MUST reject this.
    let stream_res = ZipStreamReader::new(Cursor::new(&bytes))
        .enforce_header_consistency(true)
        .visit(&mut NoopVisitor);
    assert!(
        matches!(stream_res, Err(ZipError::InvalidArchive(_))),
        "ZipStreamReader must reject shadowed malicious UnicodePath under enforce_header_consistency(true)"
    );
}

#[test]
fn f1_enforce_header_consistency_rejects_conflicting_duplicate_unicode_path() {
    let header_name = b"safe.txt";
    let crc = crc32fast::hash(header_name);

    // First-wins parsers resolve "evil.txt", while zip2's last-wins loop + get_upath see "safe.txt".
    let mut options = FullFileOptions::default().compression_method(CompressionMethod::Stored);
    options
        .add_extra_field(0x7075, make_upath_payload(crc, b"evil.txt"), false)
        .unwrap();
    options
        .add_extra_field(0x7075, make_upath_payload(crc, header_name), false)
        .unwrap();

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("safe.txt", options).unwrap();
    writer.write_all(b"payload").unwrap();
    let bytes = writer.finish().unwrap().into_inner();

    let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let res =
        archive.by_index_with_options(0, ZipReadOptions::new().enforce_header_consistency(true));
    assert!(
        matches!(res, Err(ZipError::InvalidArchive(_))),
        "Duplicate 0x7075 extra fields must be rejected under enforce_header_consistency(true)"
    );
}

// ============================================================================
// F2: Duplicate ZIP64 (0x0001) extra fields rejected under enforce_header_consistency
// ============================================================================

#[test]
fn f2_enforce_header_consistency_rejects_duplicate_zip64_in_central_directory() {
    let benign = b"benign";
    let malicious = b"evil!!";
    assert_eq!(benign.len(), malicious.len());

    let name = b"file.txt";
    let primary_len = 30 + name.len() + benign.len();
    let second_lfh = build_local_entry_bytes(name, malicious);

    // Two 0x0001 blocks in central directory:
    // #1 points to offset 0 (benign, first-wins parsers see this)
    // #2 points to offset `primary_len` (malicious, last-wins parsers see this)
    let mut cd_extra = Vec::new();
    cd_extra.extend_from_slice(&0x0001u16.to_le_bytes());
    cd_extra.extend_from_slice(&8u16.to_le_bytes());
    cd_extra.extend_from_slice(&0u64.to_le_bytes());
    cd_extra.extend_from_slice(&0x0001u16.to_le_bytes());
    cd_extra.extend_from_slice(&8u16.to_le_bytes());
    cd_extra.extend_from_slice(&(primary_len as u64).to_le_bytes());

    let mut archive_bytes = build_raw_single_entry_zip(&RawSingleEntrySpec {
        local_name: name,
        local_extra: &[],
        local_comp_size: benign.len() as u32,
        local_uncomp_size: benign.len() as u32,
        cd_name: name,
        cd_extra: &cd_extra,
        cd_comp_size: malicious.len() as u32,
        cd_uncomp_size: malicious.len() as u32,
        cd_offset: u32::MAX, // sentinel triggers ZIP64 header_start parsing
        payload: benign,
        extra_local_headers: &[second_lfh],
    });
    // Patch central directory CRC32 to match `malicious` so LFH #2 matches CD metadata.
    let cd_pos = primary_len + (30 + name.len() + malicious.len());
    let mal_crc = crc32fast::hash(malicious);
    archive_bytes[cd_pos + 16..cd_pos + 20].copy_from_slice(&mal_crc.to_le_bytes());

    let mut archive = ZipArchive::new(Cursor::new(&archive_bytes)).unwrap();
    let res =
        archive.by_index_with_options(0, ZipReadOptions::new().enforce_header_consistency(true));
    assert!(
        matches!(res, Err(ZipError::InvalidArchive(_))),
        "ZipArchive must reject duplicate 0x0001 ZIP64 extra fields under enforce_header_consistency(true)"
    );

    let mut iterable = ZipIterable::try_new(Cursor::new(&archive_bytes), Config::default())
        .unwrap()
        .enforce_header_consistency(true);
    let iter_res: ZipResult<Vec<_>> = iterable.files().unwrap().collect();
    assert!(
        matches!(iter_res, Err(ZipError::InvalidArchive(_))),
        "ZipIterable must reject duplicate 0x0001 ZIP64 extra fields under enforce_header_consistency(true)"
    );
}

#[test]
fn f2_enforce_header_consistency_rejects_duplicate_zip64_in_local_header() {
    let payload = b"hello";
    let name = b"file.txt";

    // Two 0x0001 blocks in local header, one in central directory.
    let mut one_zip64 = Vec::new();
    one_zip64.extend_from_slice(&0x0001u16.to_le_bytes());
    one_zip64.extend_from_slice(&16u16.to_le_bytes());
    one_zip64.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    one_zip64.extend_from_slice(&(payload.len() as u64).to_le_bytes());

    let mut dup_local_extra = one_zip64.clone();
    dup_local_extra.extend_from_slice(&one_zip64);

    let archive_bytes = build_raw_single_entry_zip(&RawSingleEntrySpec {
        local_name: name,
        local_extra: &dup_local_extra,
        local_comp_size: u32::MAX,
        local_uncomp_size: u32::MAX,
        cd_name: name,
        cd_extra: &one_zip64,
        cd_comp_size: u32::MAX,
        cd_uncomp_size: u32::MAX,
        cd_offset: 0,
        payload,
        extra_local_headers: &[],
    });

    let mut archive = ZipArchive::new(Cursor::new(&archive_bytes)).unwrap();
    let res =
        archive.by_index_with_options(0, ZipReadOptions::new().enforce_header_consistency(true));
    assert!(
        matches!(res, Err(ZipError::InvalidArchive(_))),
        "ZipArchive must reject duplicate 0x0001 in local header under enforce_header_consistency(true)"
    );

    let stream_res = ZipStreamReader::new(Cursor::new(&archive_bytes))
        .enforce_header_consistency(true)
        .visit(&mut NoopVisitor);
    assert!(
        matches!(stream_res, Err(ZipError::InvalidArchive(_))),
        "ZipStreamReader must reject duplicate 0x0001 in local header under enforce_header_consistency(true)"
    );
}

// ============================================================================
// F3: raw_copy_file_rename / raw_copy_file_to_path must drop stale renaming 0x7075
// ============================================================================

#[test]
fn f3_raw_copy_file_rename_and_to_path_drop_stale_unicode_path() {
    let mut src_opts = FullFileOptions::default().compression_method(CompressionMethod::Stored);
    // CRC matches target name "b.txt" (not "a.txt"), attempting to rename the copy to "evil.txt".
    src_opts
        .add_extra_field(
            0x7075,
            make_upath_payload(crc32fast::hash(b"b.txt"), b"evil.txt"),
            false,
        )
        .unwrap();

    let mut src_writer = ZipWriter::new(Cursor::new(Vec::new()));
    src_writer.start_file("a.txt", src_opts).unwrap();
    src_writer.write_all(b"hello").unwrap();
    let src_bytes = src_writer.finish().unwrap().into_inner();

    // 1. raw_copy_file_rename
    {
        let mut src = ZipArchive::new(Cursor::new(&src_bytes)).unwrap();
        let mut dst_writer = ZipWriter::new(Cursor::new(Vec::new()));
        dst_writer
            .raw_copy_file_rename(src.by_index_raw(0).unwrap(), "b.txt")
            .unwrap();
        let dst_bytes = dst_writer.finish().unwrap().into_inner();

        let mut dst = ZipArchive::new(Cursor::new(&dst_bytes)).unwrap();
        let names: Vec<String> = dst.file_names().map(|n| n.unwrap().into_owned()).collect();
        assert_eq!(
            names,
            ["b.txt"],
            "raw_copy_file_rename must not be renamed by stale 0x7075"
        );
        // Also verify local header dropped the renaming 0x7075 (header consistency must pass).
        dst.by_name_with_options(
            "b.txt",
            ZipReadOptions::new().enforce_header_consistency(true),
        )
        .expect("copied entry local and central headers must stay consistent");
    }

    // 2. raw_copy_file_to_path
    {
        let mut src = ZipArchive::new(Cursor::new(&src_bytes)).unwrap();
        let mut dst_writer = ZipWriter::new(Cursor::new(Vec::new()));
        dst_writer
            .raw_copy_file_to_path(src.by_index_raw(0).unwrap(), std::path::Path::new("b.txt"))
            .unwrap();
        let dst_bytes = dst_writer.finish().unwrap().into_inner();

        let mut dst = ZipArchive::new(Cursor::new(&dst_bytes)).unwrap();
        let names: Vec<String> = dst.file_names().map(|n| n.unwrap().into_owned()).collect();
        assert_eq!(
            names,
            ["b.txt"],
            "raw_copy_file_to_path must not be renamed by stale 0x7075"
        );
        dst.by_name_with_options(
            "b.txt",
            ZipReadOptions::new().enforce_header_consistency(true),
        )
        .expect("copied entry local and central headers must stay consistent");
    }
}

// ============================================================================
// F4: AES extra field update past 4 GiB must not truncate offset to 32 bits
// ============================================================================

#[cfg(feature = "aes-crypto")]
#[test]
fn f4_aes_entry_past_4_gib_updates_extra_field_at_u64_offset() {
    let shifted = Shifted {
        pos: BASE,
        ..Shifted::default()
    };
    let mut writer = ZipWriter::new(shifted);
    let opts = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .with_aes_encryption(zip::AesMode::Aes256, "secret");
    writer.start_file("secret.txt", opts).unwrap();
    writer
        .write_all(b"01234567890123456789extra-bytes-for-ae1")
        .unwrap();
    let shifted = writer.finish().unwrap();

    let mut archive = ZipArchive::new(shifted).unwrap();
    let mut file = archive.by_name_decrypt("secret.txt", b"secret").unwrap();
    let mut out = Vec::new();
    file.read_to_end(&mut out).unwrap();
    assert_eq!(out, b"01234567890123456789extra-bytes-for-ae1");
}

// ============================================================================
// F5: replace_zip64_extra_field deduplicates all 0x0001 blocks (incl. Custom(0x0001))
// ============================================================================

#[test]
fn f5_new_append_and_merge_archive_deduplicate_existing_zip64_fields() {
    let payload = b"hello";
    let name = b"dup.txt";

    // Build an archive that already carries TWO 0x0001 extra fields in its central directory
    // (simulating an archive produced by zip 9.7/9.8 or a crafted input).
    let mut one_zip64 = Vec::new();
    one_zip64.extend_from_slice(&0x0001u16.to_le_bytes());
    one_zip64.extend_from_slice(&16u16.to_le_bytes());
    one_zip64.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    one_zip64.extend_from_slice(&(payload.len() as u64).to_le_bytes());

    let mut dup_cd_extra = one_zip64.clone();
    dup_cd_extra.extend_from_slice(&one_zip64);

    let raw_archive = build_raw_single_entry_zip(&RawSingleEntrySpec {
        local_name: name,
        local_extra: &one_zip64,
        local_comp_size: u32::MAX,
        local_uncomp_size: u32::MAX,
        cd_name: name,
        cd_extra: &dup_cd_extra,
        cd_comp_size: u32::MAX,
        cd_uncomp_size: u32::MAX,
        cd_offset: 0,
        payload,
        extra_local_headers: &[],
    });

    // Confirm input actually parsed 2 ZIP64 extra fields.
    {
        let mut src = ZipArchive::new(Cursor::new(&raw_archive)).unwrap();
        let count = src
            .by_name("dup.txt")
            .unwrap()
            .extra_data_fields()
            .filter(|f| matches!(f, ExtraField::Zip64ExtendedInformation(_)))
            .count();
        assert_eq!(
            count, 2,
            "precondition: input archive has two 0x0001 blocks"
        );
    }

    // 1. new_append must rewrite central directory with exactly ONE 0x0001 block.
    {
        let writer = ZipWriter::new_append(Cursor::new(raw_archive.clone())).unwrap();
        let rewritten = writer.finish().unwrap().into_inner();
        let mut archive = ZipArchive::new(Cursor::new(&rewritten)).unwrap();
        let count = archive
            .by_name("dup.txt")
            .unwrap()
            .extra_data_fields()
            .filter(|f| matches!(f, ExtraField::Zip64ExtendedInformation(_)))
            .count();
        assert_eq!(
            count, 1,
            "new_append must deduplicate existing 0x0001 fields"
        );
    }

    // 2. merge_archive into >4 GiB writer must also emit exactly ONE 0x0001 block and read back.
    {
        let src = ZipArchive::new(Cursor::new(raw_archive)).unwrap();
        let shifted = Shifted {
            pos: BASE,
            ..Shifted::default()
        };
        let mut writer = ZipWriter::new(shifted);
        writer.merge_archive(src).unwrap();
        let merged = writer.finish().unwrap();

        let mut archive = ZipArchive::new(merged).unwrap();
        let count = archive
            .by_name("dup.txt")
            .unwrap()
            .extra_data_fields()
            .filter(|f| matches!(f, ExtraField::Zip64ExtendedInformation(_)))
            .count();
        assert_eq!(
            count, 1,
            "merge_archive must deduplicate existing 0x0001 fields"
        );
        let mut read = Vec::new();
        archive
            .by_name("dup.txt")
            .unwrap()
            .read_to_end(&mut read)
            .unwrap();
        assert_eq!(read, payload);
    }
}

#[test]
fn f5_add_extra_field_zip64_does_not_emit_duplicate_zip64_blocks() {
    let mut opts = FullFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(true);
    // Caller manually supplies a 0x0001 block via add_extra_field.
    opts.add_extra_field(0x0001, [0u8; 16], false).unwrap();

    let shifted = Shifted {
        pos: BASE,
        ..Shifted::default()
    };
    let mut writer = ZipWriter::new(shifted);
    writer.start_file("f.bin", opts).unwrap();
    writer.write_all(b"content").unwrap();
    let mut shifted = writer.finish().unwrap();

    // Local header must have at most 1 ZIP64 block.
    assert_eq!(
        count_local_extra_id(&mut shifted, BASE, 0x0001),
        1,
        "local header must not contain duplicate 0x0001 blocks"
    );

    // Central directory must have exactly 1 ZIP64 block and read back cleanly.
    let mut archive = ZipArchive::new(shifted).unwrap();
    let count = archive
        .by_name("f.bin")
        .unwrap()
        .extra_data_fields()
        .filter(|f| matches!(f, ExtraField::Zip64ExtendedInformation(_)))
        .count();
    assert_eq!(
        count, 1,
        "central header must not contain duplicate 0x0001 blocks"
    );

    let mut read = Vec::new();
    archive
        .by_name("f.bin")
        .unwrap()
        .read_to_end(&mut read)
        .unwrap();
    assert_eq!(read, b"content");

    // With validate_extra_fields(true), raw custom 0x0001 must be rejected upfront.
    let mut validated = FullFileOptions::default().validate_extra_fields(true);
    assert!(
        validated
            .add_extra_field(0x0001, [0xAA, 0xBB, 0xCC], false)
            .is_err(),
        "validate_extra_fields(true) must reject writer-managed/malformed 0x0001"
    );
}

// ============================================================================
// F6: get_data_descriptor past 4 GiB for small entry must read 16-byte descriptor
// ============================================================================

#[test]
fn f6_data_descriptor_past_4_gib_for_small_entry_reads_32_bit_descriptor() {
    let shifted = Shifted {
        pos: BASE,
        ..Shifted::default()
    };
    let mut writer = ZipWriter::new(shifted);
    // ZipCrypto forces UsingDataDescriptor; file is tiny (< 4 GiB) so writer writes 16-byte DD,
    // while header_start >= BASE puts an offset-only 0x0001 in central directory.
    let opts = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .with_deprecated_encryption(b"pw")
        .unwrap();
    writer.start_file("small.bin", opts).unwrap();
    writer.write_all(b"hello world").unwrap();
    // Add a second entry immediately after so over-reading 24 bytes corrupts uncompressed_size
    // instead of hitting EOF.
    writer
        .start_file(
            "next.bin",
            SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(b"next").unwrap();
    let shifted = writer.finish().unwrap();

    let mut archive = ZipArchive::new(shifted).unwrap();
    let entry = archive.by_index_with_data_descriptor(0).unwrap();
    let expected_crc = entry.crc32();
    let expected_comp = entry.compressed_size();
    let expected_uncomp = entry.size();
    match entry.data_descriptor() {
        Some(ZipDataDescriptor::ZipDataDescriptorBlock(dd)) => {
            assert_eq!(dd.crc32, expected_crc);
            assert_eq!(u64::from(dd.compressed_size), expected_comp);
            assert_eq!(u64::from(dd.uncompressed_size), expected_uncomp);
        }
        other => panic!("expected 32-bit ZipDataDescriptorBlock, got {other:?}"),
    }
}

// ============================================================================
// F7: Zip64ExtendedInformation::parse must reject truncated trailing bytes
// ============================================================================

#[test]
fn f7_truncated_zip64_extra_field_trailing_bytes_rejected() {
    let payload = b"hello";
    let name = b"trunc.txt";

    // Declare 0x0001 with length = 32 (16 bytes sizes + 16 trailing bytes),
    // but only supply 16 payload bytes in the extra field buffer (truncated!).
    let mut trunc_zip64 = Vec::new();
    trunc_zip64.extend_from_slice(&0x0001u16.to_le_bytes());
    trunc_zip64.extend_from_slice(&32u16.to_le_bytes()); // claims 32 bytes
    trunc_zip64.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    trunc_zip64.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    // Missing the remaining 16 bytes!

    let archive_bytes = build_raw_single_entry_zip(&RawSingleEntrySpec {
        local_name: name,
        local_extra: &[],
        local_comp_size: payload.len() as u32,
        local_uncomp_size: payload.len() as u32,
        cd_name: name,
        cd_extra: &trunc_zip64,
        cd_comp_size: u32::MAX,
        cd_uncomp_size: u32::MAX,
        cd_offset: 0,
        payload,
        extra_local_headers: &[],
    });

    let res = ZipArchive::new(Cursor::new(&archive_bytes));
    assert!(
        matches!(res, Err(ZipError::InvalidArchive(_))),
        "Truncated ZIP64 extra field trailing bytes must be rejected, got: {:?}",
        res.map(|a| a.len())
    );
}

// ============================================================================
// Duplicate AES (0x9901) extra fields rejected under enforce_header_consistency
// ============================================================================

#[test]
fn duplicate_aes_extra_field_in_local_and_central_headers_rejected() {
    let payload = b"hello";
    let name = b"dup_aes.txt";

    // Two valid 11-byte AES (0x9901) extra fields (AE-2, "AE", AES-256, Stored).
    let one_aes: [u8; 11] = [
        0x01, 0x99, 0x07, 0x00, 0x02, 0x00, b'A', b'E', 0x03, 0x00, 0x00,
    ];
    let mut dup_aes = Vec::with_capacity(one_aes.len() * 2);
    dup_aes.extend_from_slice(&one_aes);
    dup_aes.extend_from_slice(&one_aes);

    // 1. Duplicate 0x9901 in local header only (central header has none).
    let local_dup_archive = build_raw_single_entry_zip(&RawSingleEntrySpec {
        local_name: name,
        local_extra: &dup_aes,
        local_comp_size: payload.len() as u32,
        local_uncomp_size: payload.len() as u32,
        cd_name: name,
        cd_extra: &[],
        cd_comp_size: payload.len() as u32,
        cd_uncomp_size: payload.len() as u32,
        cd_offset: 0,
        payload,
        extra_local_headers: &[],
    });

    let mut archive = ZipArchive::new(Cursor::new(&local_dup_archive)).unwrap();
    let res =
        archive.by_index_with_options(0, ZipReadOptions::new().enforce_header_consistency(true));
    assert!(
        matches!(res, Err(ZipError::InvalidArchive(_))),
        "ZipArchive must reject duplicate 0x9901 in local header under enforce_header_consistency(true)"
    );

    let mut iterable = ZipIterable::try_new(Cursor::new(&local_dup_archive), Config::default())
        .unwrap()
        .enforce_header_consistency(true);
    let iter_res: ZipResult<Vec<_>> = iterable.files().unwrap().collect();
    assert!(
        matches!(iter_res, Err(ZipError::InvalidArchive(_))),
        "ZipIterable must reject duplicate 0x9901 in local header under enforce_header_consistency(true)"
    );

    let stream_res = ZipStreamReader::new(Cursor::new(&local_dup_archive))
        .enforce_header_consistency(true)
        .visit(&mut NoopVisitor);
    assert!(
        matches!(stream_res, Err(ZipError::InvalidArchive(_))),
        "ZipStreamReader must reject duplicate 0x9901 in local header under enforce_header_consistency(true)"
    );

    // 2. Duplicate 0x9901 in central directory.
    let cd_dup_archive = build_raw_single_entry_zip(&RawSingleEntrySpec {
        local_name: name,
        local_extra: &[],
        local_comp_size: payload.len() as u32,
        local_uncomp_size: payload.len() as u32,
        cd_name: name,
        cd_extra: &dup_aes,
        cd_comp_size: payload.len() as u32,
        cd_uncomp_size: payload.len() as u32,
        cd_offset: 0,
        payload,
        extra_local_headers: &[],
    });

    match ZipArchive::new(Cursor::new(&cd_dup_archive)) {
        // With `aes-crypto`, `apply_extra_fields` rejects duplicate 0x9901 during CD parse.
        Err(ZipError::InvalidArchive(_)) => {}
        // Without `aes-crypto`, strict consistency validation must still reject duplicate 0x9901.
        Ok(mut archive) => {
            let res = archive
                .by_index_with_options(0, ZipReadOptions::new().enforce_header_consistency(true));
            assert!(
                matches!(res, Err(ZipError::InvalidArchive(_))),
                "ZipArchive must reject duplicate 0x9901 in central directory under enforce_header_consistency(true)"
            );
        }
        Err(other) => panic!("unexpected error opening archive: {other:?}"),
    }
}
