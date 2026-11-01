//! Code related to stream reading

use crate::read::ZipFileDataReadExt;
use crate::read::central_header_to_zip_file_inner;
use crate::read::{ZipFile, ZipFileEntry};
use zip_core_lib::ZipReadOptions;
use zip_core_lib::extra_fields::ExtraFields;
use zip_core_lib::format::blocks::{
    FixedSizeBlock, Pod, Zip32CDEBlock, Zip32CentralDirectoryEnd, Zip64CDEBlock,
    Zip64CDELocatorBlock, ZipCentralEntryBlock, ZipLocalEntryBlock,
};
use zip_core_lib::format::magic::Magic;
use zip_core_lib::read::HeaderConsistencyMetadata;
use zip_core_lib::result::{ZipError, ZipResult, invalid};
use zip_core_lib::types::ZipFileData;

use crate::read::stream::ReadFromStreamResult::{
    CentralDirectoryHeader, EndOfCentralDirectory, File,
};
use std::borrow::Cow;
use std::io::{self, Cursor, Read};

/// Stream decoder for zip.
#[derive(Debug)]
pub struct ZipStreamReader<R> {
    reader: R,
    enforce_header_consistency: bool,
}

impl<R> ZipStreamReader<R> {
    /// Create a new `ZipStreamReader` with default settings. For backward-compatibility,
    /// this doesn't check that the local headers match the central directory.
    pub const fn new(reader: R) -> Self {
        Self {
            reader,
            enforce_header_consistency: false,
        }
    }

    /// Configure whether to validate consistency between Local File Headers (LFH)
    /// and Central Directory File Headers (CDFH).
    ///
    /// When enabled, any discrepancy (such as Zip Cloaking / Dual-Header Smuggling attacks
    /// where an attacker provides different names or hidden files across headers, or an extra
    /// end-of-central-directory record that selects a different directory) will cause the
    /// extraction to abort with [`ZipError::InvalidArchive`].
    ///
    /// The headers can only be compared once the central directory, at the end of the stream,
    /// has been read. The `ZipStreamExtract` extension trait therefore extracts into a temporary directory
    /// inside the destination and moves the entries into place only if the check passes. A
    /// [`ZipStreamVisitor`] passed to [`ZipStreamReader::visit`] sees every entry before the
    /// check, so it must not act on them until `visit` returns `Ok`.
    #[must_use]
    pub const fn enforce_header_consistency(mut self, enforce: bool) -> Self {
        self.enforce_header_consistency = enforce;
        self
    }

    #[doc(hidden)]
    pub const fn header_consistency_enforced(&self) -> bool {
        self.enforce_header_consistency
    }
}

fn parse_central_directory<R: Read>(reader: &mut R) -> ZipResult<ZipFileEntry<'static>> {
    // Give archive_offset and central_header_start dummy value 0, since
    // they are not used in the output.
    let archive_offset = 0;
    let central_header_start = 0;

    // Parse central header
    let block = ZipCentralEntryBlock::parse(reader)?;
    let (file, file_name_raw) =
        central_header_to_zip_file_inner(reader, archive_offset, central_header_start, block)?;
    Ok(ZipFileEntry {
        file_name_raw: Cow::Owned(file_name_raw),
        data: Cow::Owned(file),
    })
}

/// Check the records after the central directory before trusting the streamed entries.
fn validate_stream_footer<R: Read>(
    reader: &mut R,
    mut signature: [u8; 4],
    entry_count: usize,
    central_start: u64,
    central_size: u64,
    footer_offset: u64,
) -> ZipResult<()> {
    let mut zip64_count = None;
    if Magic::from_le_bytes(signature) == Magic::ZIP64_CENTRAL_DIRECTORY_END_SIGNATURE {
        let record = Zip64CDEBlock::parse(&mut (&signature[..]).chain(&mut *reader))?;
        // The record size excludes its signature and size field. The fixed fields after
        // those two occupy 44 bytes; skip the optional extensible data without allocating.
        if record.record_size < 44 {
            return Err(invalid!("Invalid ZIP64 end record size"));
        }
        let extra_len = record.record_size - 44;
        if io::copy(&mut (&mut *reader).take(extra_len), &mut io::sink())? != extra_len {
            return Err(invalid!("Truncated ZIP64 end record"));
        }
        if record.disk_number != 0
            || record.disk_with_central_directory != 0
            || record.number_of_files_on_this_disk != record.number_of_files
            || record.number_of_files != entry_count as u64
            || record.central_directory_size != central_size
            || record.central_directory_offset != central_start
        {
            return Err(invalid!("Inconsistent ZIP64 end record"));
        }
        zip64_count = Some(record.number_of_files);

        reader.read_exact(&mut signature)?;
        if Magic::from_le_bytes(signature) != Magic::ZIP64_CENTRAL_DIRECTORY_END_LOCATOR_SIGNATURE {
            return Err(invalid!("Missing ZIP64 end record locator"));
        }
        let locator = Zip64CDELocatorBlock::parse(&mut (&signature[..]).chain(&mut *reader))?;
        if locator.disk_with_central_directory != 0 || locator.number_of_disks != 1 {
            return Err(invalid!("Multi-disk ZIP files are not supported"));
        }
        if locator.end_of_central_directory_offset != footer_offset {
            return Err(invalid!("Inconsistent ZIP64 end record locator offset"));
        }
        reader.read_exact(&mut signature)?;
    }

    if Magic::from_le_bytes(signature) != Magic::CENTRAL_DIRECTORY_END_SIGNATURE {
        return Err(invalid!("Missing end of central directory record"));
    }
    let end = Zip32CentralDirectoryEnd::parse(&mut (&signature[..]).chain(reader))?;
    if end.disk_number != 0
        || end.disk_with_central_directory != 0
        || end.number_of_files_on_this_disk != end.number_of_files
        || (end.number_of_files != u16::MAX && usize::from(end.number_of_files) != entry_count)
        || (end.central_directory_size != u32::MAX
            && u64::from(end.central_directory_size) != central_size)
        || (end.central_directory_offset != u32::MAX
            && u64::from(end.central_directory_offset) != central_start)
        || (zip64_count.is_none()
            && (end.number_of_files == u16::MAX
                || end.central_directory_size == u32::MAX
                || end.central_directory_offset == u32::MAX))
    {
        return Err(invalid!("Inconsistent end of central directory record"));
    }
    Ok(())
}

fn contains_unlisted_local_header_or_extra_eocd<R: Read>(
    reader: &mut R,
    scanned_start: u64,
    central_start: u64,
    central_size: u64,
    entry_count: usize,
) -> ZipResult<bool> {
    const LOCAL_FILE_HEADER_SIGNATURE: [u8; 4] = Magic::LOCAL_FILE_HEADER_SIGNATURE.to_le_bytes();
    const EOCD_SIGNATURE: [u8; 4] = Magic::CENTRAL_DIRECTORY_END_SIGNATURE.to_le_bytes();
    const FIXED_HEADER_LEN: usize = 4 + std::mem::size_of::<ZipLocalEntryBlock>();
    const EOCD_FIXED_LEN: usize = 4 + std::mem::size_of::<Zip32CDEBlock>();
    const READ_BUFFER_LEN: usize = 8192;

    let mut buffer = Vec::with_capacity(READ_BUFFER_LEN);
    let mut buffer_start = scanned_start;
    let mut search_from = 0;
    let mut eof = false;

    loop {
        let search = &buffer[search_from..];
        let local_candidate = search
            .windows(LOCAL_FILE_HEADER_SIGNATURE.len())
            .position(|window| window == LOCAL_FILE_HEADER_SIGNATURE)
            .map(|offset| offset + search_from);
        let eocd_candidate = search
            .windows(EOCD_SIGNATURE.len())
            .position(|window| window == EOCD_SIGNATURE)
            .map(|offset| offset + search_from);

        if let Some(offset) =
            local_candidate.filter(|&local| eocd_candidate.is_none_or(|eocd| local <= eocd))
        {
            let fixed_end = offset + FIXED_HEADER_LEN;
            while buffer.len() < fixed_end && !eof {
                let mut chunk = [0; READ_BUFFER_LEN];
                let read = reader.read(&mut chunk)?;
                if read == 0 {
                    eof = true;
                } else {
                    buffer.extend_from_slice(&chunk[..read]);
                }
            }
            if buffer.len() < fixed_end {
                search_from = offset + 1;
                continue;
            }

            let block =
                match ZipLocalEntryBlock::parse(&mut Cursor::new(&buffer[offset..fixed_end])) {
                    Ok(block) => block,
                    Err(_) => {
                        search_from = offset + 1;
                        continue;
                    }
                };
            let header_end = fixed_end
                + usize::from(block.file_name_length)
                + usize::from(block.extra_field_length);
            while buffer.len() < header_end && !eof {
                let mut chunk = [0; READ_BUFFER_LEN];
                let read = reader.read(&mut chunk)?;
                if read == 0 {
                    eof = true;
                } else {
                    buffer.extend_from_slice(&chunk[..read]);
                }
            }
            if buffer.len() < header_end {
                search_from = offset + 1;
                continue;
            }

            return Ok(true);
        }

        if let Some(offset) = eocd_candidate {
            let fixed_end = offset + EOCD_FIXED_LEN;
            while buffer.len() < fixed_end && !eof {
                let mut chunk = [0; READ_BUFFER_LEN];
                let read = reader.read(&mut chunk)?;
                if read == 0 {
                    eof = true;
                } else {
                    buffer.extend_from_slice(&chunk[..read]);
                }
            }
            if buffer.len() < fixed_end {
                search_from = offset + 1;
                continue;
            }

            let record = match Zip32CDEBlock::parse(&mut Cursor::new(&buffer[offset..fixed_end])) {
                Ok(record) => record,
                Err(_) => {
                    search_from = offset + 1;
                    continue;
                }
            };
            let comment_end = fixed_end + usize::from(record.zip_file_comment_length);
            while buffer.len() < comment_end && !eof {
                let mut chunk = [0; READ_BUFFER_LEN];
                let read = reader.read(&mut chunk)?;
                if read == 0 {
                    eof = true;
                } else {
                    buffer.extend_from_slice(&chunk[..read]);
                }
            }
            if buffer.len() < comment_end {
                search_from = offset + 1;
                continue;
            }

            let candidate_offset = buffer_start + offset as u64;
            let is_zip64 = record.number_of_files_on_this_disk == u16::MAX
                || record.number_of_files == u16::MAX
                || record.central_directory_size == u32::MAX
                || record.central_directory_offset == u32::MAX;
            let fields_are_plausible = record.disk_number == 0
                && record.disk_with_central_directory == 0
                && record.number_of_files_on_this_disk == record.number_of_files
                && (is_zip64
                    || (u64::from(record.number_of_files)
                        <= u64::from(record.central_directory_size) / 46
                        && u64::from(record.central_directory_offset)
                            .checked_add(u64::from(record.central_directory_size))
                            .is_some_and(|end| end <= candidate_offset)));
            let candidate_central_start =
                candidate_offset.checked_sub(u64::from(record.central_directory_size));
            let archive_offset_is_valid = candidate_central_start
                .is_some_and(|start| start >= u64::from(record.central_directory_offset));
            let same_central_directory = candidate_central_start == Some(central_start)
                && u64::from(record.central_directory_size) == central_size
                && usize::from(record.number_of_files) == entry_count;
            if fields_are_plausible
                && (is_zip64 || (archive_offset_is_valid && !same_central_directory))
            {
                return Ok(true);
            }
            search_from = offset + 1;
            continue;
        }

        if eof {
            return Ok(false);
        }
        if buffer.len() > LOCAL_FILE_HEADER_SIGNATURE.len() - 1 {
            let retain_from = buffer.len() - (LOCAL_FILE_HEADER_SIGNATURE.len() - 1);
            buffer.drain(..retain_from);
            buffer_start += retain_from as u64;
            search_from = 0;
        }
        let mut chunk = [0; READ_BUFFER_LEN];
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            eof = true;
        } else {
            buffer.extend_from_slice(&chunk[..read]);
        }
    }
}

impl<R: Read> ZipStreamReader<R> {
    /// Iterate over the stream and extract all file and their
    /// metadata.
    pub fn visit<V: ZipStreamVisitor>(self, visitor: &mut V) -> ZipResult<()> {
        struct CountingReader<R> {
            reader: R,
            count: u64,
        }

        impl<R: Read> Read for CountingReader<R> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = self.reader.read(buf)?;
                self.count += n as u64;
                Ok(n)
            }
        }

        struct LocalEntryMetadata {
            metadata: HeaderConsistencyMetadata<Box<[u8]>>,
            stream_offset: u64,
        }

        struct CentralEntryMetadata {
            metadata: HeaderConsistencyMetadata<Box<[u8]>>,
            header_start: u64,
        }

        let mut reader = CountingReader {
            reader: self.reader,
            count: 0,
        };

        let mut local_entries = Vec::new();

        let has_central_directory = loop {
            let offset_before_read = reader.count;
            let read_from_stream_result =
                read_zipfile_from_stream_with_options_internal(&mut reader, Default::default())?;
            match read_from_stream_result {
                File(mut file) => {
                    if self.enforce_header_consistency {
                        local_entries.push(LocalEntryMetadata {
                            metadata: file
                                .data
                                .consistency_metadata(file.name_raw(), true)
                                .into_owned(),
                            stream_offset: offset_before_read,
                        });
                    }
                    visitor.visit_file(&mut file)?;
                }
                CentralDirectoryHeader => break true,
                EndOfCentralDirectory => break false,
            }
        };

        let mut central_entries = Vec::new();
        let mut footer = None;

        if has_central_directory {
            let central_start = reader.count - 4;
            let mut signature = Magic::CENTRAL_DIRECTORY_HEADER_SIGNATURE.to_le_bytes();
            loop {
                let mut central_directory = (&signature[..]).chain(&mut reader);
                let metadata = match parse_central_directory(&mut central_directory) {
                    Ok(metadata) => metadata,
                    Err(e) if self.enforce_header_consistency => return Err(e),
                    Err(_) => break,
                };
                if self.enforce_header_consistency {
                    central_entries.push(CentralEntryMetadata {
                        metadata: metadata
                            .data
                            .consistency_metadata(metadata.name_raw(), false)
                            .into_owned(),
                        header_start: metadata.header_start(),
                    });
                }
                visitor.visit_additional_metadata(&metadata)?;
                if let Err(e) = reader.read_exact(&mut signature) {
                    if self.enforce_header_consistency {
                        return Err(e.into());
                    }
                    break;
                }
                if Magic::from_le_bytes(signature) != Magic::CENTRAL_DIRECTORY_HEADER_SIGNATURE {
                    break;
                }
            }
            if self.enforce_header_consistency {
                // The ZIP central directory may end with one optional digital-signature
                // record. It is part of the declared central-directory size. Skip its
                // opaque bytes without trusting or allocating from them, then validate
                // the footer at its actual position.
                if Magic::from_le_bytes(signature) == Magic::CENTRAL_DIRECTORY_DIGITAL_SIGNATURE {
                    let mut len = [0u8; 2];
                    reader.read_exact(&mut len)?;
                    let len = u64::from(u16::from_le_bytes(len));
                    if io::copy(&mut (&mut reader).take(len), &mut io::sink())? != len {
                        return Err(invalid!("Truncated central directory digital signature"));
                    }
                    reader.read_exact(&mut signature)?;
                }
                let central_size = reader.count - 4 - central_start;
                footer = Some((signature, central_start, central_size, reader.count - 4));
            }
        } else if self.enforce_header_consistency {
            footer = Some((
                Magic::CENTRAL_DIRECTORY_END_SIGNATURE.to_le_bytes(),
                reader.count - 4,
                0,
                reader.count - 4,
            ));
        }

        if self.enforce_header_consistency {
            if local_entries.len() != central_entries.len() {
                return Err(invalid!(
                    "header inconsistency detected: entry count mismatch ({} local vs {} central)",
                    local_entries.len(),
                    central_entries.len()
                ));
            }

            // Sort central directory entries by their local header offset (header_start)
            // to align with the sequential physical stream order of the local headers.
            central_entries.sort_by_key(|e| e.header_start);

            let mut seen_names = std::collections::HashSet::with_capacity(central_entries.len());

            for (i, (local, central)) in
                local_entries.iter().zip(central_entries.iter()).enumerate()
            {
                // Ensure strictly monotonic local header offsets in central directory
                if i > 0 && central.header_start <= central_entries[i - 1].header_start {
                    return Err(invalid!(
                        "header inconsistency detected: two central directory entries for same file"
                    ));
                }

                // Verify that the local header offset matches the physical stream offset exactly
                // The stream starts at its first local header. A common shift in all
                // central offsets is still an inconsistency, even if their order matches.
                if central.header_start != local.stream_offset {
                    return Err(invalid!(
                        "header inconsistency detected: local header offset mismatch for '{}' (expected {}, central claims {})",
                        String::from_utf8_lossy(&local.metadata.name),
                        local.stream_offset,
                        central.header_start
                    ));
                }

                local.metadata.validate_matches(&central.metadata)?;

                if !seen_names.insert(&central.metadata.name) {
                    return Err(invalid!(
                        "header inconsistency detected: duplicate file name '{}'",
                        String::from_utf8_lossy(&central.metadata.name)
                    ));
                }
            }
            let (signature, central_start, central_size, footer_offset) =
                footer.ok_or(invalid!("Missing end of central directory record"))?;
            validate_stream_footer(
                &mut reader,
                signature,
                central_entries.len(),
                central_start,
                central_size,
                footer_offset,
            )?;
            let scan_start = reader.count;
            if contains_unlisted_local_header_or_extra_eocd(
                &mut reader,
                scan_start,
                central_start,
                central_size,
                central_entries.len(),
            )? {
                return Err(invalid!(
                    "header inconsistency detected: extra local file header or ambiguous end of central directory"
                ));
            }
        }

        Ok(())
    }
}

pub trait ZipStreamVisitor {
    ///  * `file` - contains the content of the file and most of the metadata,
    ///    except:
    ///     - `comment`: set to an empty string
    ///     - `data_start`: set to 0
    ///     - `external_attributes`: `unix_mode()`: will return None
    fn visit_file<R: Read>(&mut self, file: &mut ZipFile<'_, R>) -> ZipResult<()>;

    /// This function is guaranteed to be called after all `visit_file`s.
    ///
    ///  * `metadata` - Provides missing metadata in `visit_file`.
    fn visit_additional_metadata(&mut self, metadata: &ZipFileEntry<'_>) -> ZipResult<()>;
}

/// Read `ZipFile` structures from a non-seekable reader.
///
/// This is an alternative method to read a zip file. If possible, use the `ZipArchive` functions
/// as some information will be missing when reading this manner.
///
/// Reads a file header from the start of the stream. Will return `Ok(Some(..))` if a file is
/// present at the start of the stream. Returns `Ok(None)` if the start of the central directory
/// is encountered. No more files should be read after this.
///
/// The Drop implementation of `ZipFile` ensures that the reader will be correctly positioned after
/// the structure is done.
///
/// Missing fields are:
/// * `comment`: set to an empty string
/// * `data_start`: set to 0
/// * `external_attributes`: `unix_mode()`: will return None
pub fn read_zipfile_from_stream<R: Read>(reader: &mut R) -> ZipResult<Option<ZipFile<'_, R>>> {
    read_zipfile_from_stream_with_options(reader, ZipReadOptions::default())
}

/// Read `ZipFile` from a non-seekable reader like [`read_zipfile_from_stream`] does, but assume the
/// given compressed size and don't read any further ahead than that.
pub fn read_zipfile_from_stream_with_compressed_size<R: Read>(
    reader: &mut R,
    compressed_size: u64,
) -> ZipResult<Option<ZipFile<'_, R>>> {
    let options = ZipReadOptions::default().override_compressed_size(compressed_size);
    read_zipfile_from_stream_with_options(reader, options)
}

/// Same as `read_zipfile_from_stream` but with `ZipReadOptions`
/// Since LZMA decoding requires the uncompressed length, you will need to override it
pub fn read_zipfile_from_stream_with_options<'a, R: Read>(
    reader: &'a mut R,
    options: ZipReadOptions<'a>,
) -> ZipResult<Option<ZipFile<'a, R>>> {
    let result = read_zipfile_from_stream_with_options_internal(reader, options)?;
    match result {
        ReadFromStreamResult::File(file) => Ok(Some(file)),
        ReadFromStreamResult::CentralDirectoryHeader => Ok(None),
        ReadFromStreamResult::EndOfCentralDirectory => Ok(None),
    }
}

enum ReadFromStreamResult<'a, R: Read> {
    File(ZipFile<'a, R>),
    CentralDirectoryHeader,
    EndOfCentralDirectory,
}

/// Returns Ok(Ok(file)) if a file was read, Ok(Err(true)) if a central directory entry signature was found,
/// and Err(false) if an EOCD signature was found.
fn read_zipfile_from_stream_with_options_internal<'a, R: Read>(
    reader: &'a mut R,
    mut options: ZipReadOptions<'a>,
) -> ZipResult<ReadFromStreamResult<'a, R>> {
    // We can't use the typical [`ZipLocalEntryBlock::parse`] method, as we follow separate code paths depending on the
    // "magic" value (since the magic value will be from the central directory header if we've
    // finished iterating over all the actual files).
    let mut magic_buf = [0; size_of::<u32>()];
    reader.read_exact(&mut magic_buf)?;

    match Magic::from_le_bytes(magic_buf) {
        Magic::LOCAL_FILE_HEADER_SIGNATURE => (),
        Magic::CENTRAL_DIRECTORY_HEADER_SIGNATURE => return Ok(CentralDirectoryHeader),
        Magic::CENTRAL_DIRECTORY_END_SIGNATURE => return Ok(EndOfCentralDirectory), // Empty archive may have no CENTRAL_DIRECTORY_HEADER_SIGNATURE
        _ => return Err(ZipLocalEntryBlock::WRONG_MAGIC_ERROR),
    }

    let mut block = ZipLocalEntryBlock::zeroed();
    reader.read_exact(block.as_bytes_mut())?;

    let block = block.from_le();
    // parse file_name_raw
    let file_name_length: usize = block.file_name_length.into();
    let mut file_name_raw = vec![0u8; file_name_length];
    if let Err(e) = reader.read_exact(&mut file_name_raw) {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            return Err(invalid!("File name extends beyond file boundary"));
        }
        return Err(e.into());
    }

    // parse extra fields raw
    let extra_field_length: usize = block.extra_field_length.into();
    let mut extra_fields_raw = vec![0u8; extra_field_length];
    if let Err(e) = reader.read_exact(&mut extra_fields_raw) {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            return Err(invalid!("Extra field extends beyond file boundary"));
        }
        return Err(e.into());
    }
    // parse extra fields
    let extra_fields = ExtraFields::parse(&extra_fields_raw, &block, true)?;
    let mut data = ZipFileData::from_local_block(block, extra_fields)?;
    data.apply_extra_fields(&mut file_name_raw)?;
    if data.is_using_data_descriptor() {
        if let Some(comp_size) = options.forced_compressed_size() {
            data.compressed_size = comp_size;
        } else {
            return Err(ZipError::UnsupportedArchive(
                "The file length is not available in the local header",
            ));
        }
    }
    if let Some(uncomp_size) = options.forced_uncompressed_size() {
        data.uncompressed_size = uncomp_size;
    } else if data.is_using_data_descriptor() {
        // The local header doesn't carry the size; it follows the data.
        options.size_unknown = true;
    }
    if let Some(crc) = options.forced_crc() {
        data.crc32 = crc;
    }

    let limit_reader = reader.take(data.compressed_size);
    let reader = data.make_reader(limit_reader, options)?;
    Ok(File(ZipFile {
        file_name_raw: Cow::Owned(file_name_raw),
        data: Cow::Owned(data),
        reader,
    }))
}
