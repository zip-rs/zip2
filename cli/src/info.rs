use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
};

use zip::read::ZipArchive;

use crate::{
    args::{extract::InputSpec, info::*},
    extract::{
        entries::{IterateEntries, StreamInput, ZipFileInput},
        matcher::{CompiledMatcher, EntryMatcher},
        receiver::EntryData,
    },
    CommandError, WrapCommandErr,
};

mod directives;
mod formats;
use directives::{
    archive::{
        compiled::{CompiledArchiveDirective, CompiledArchiveFormat},
        ArchiveData,
    },
    compiled::CompiledFormatSpec,
    entry::compiled::{CompiledEntryDirective, CompiledEntryFormat},
};

pub struct ArchiveWithPath {
    pub path: PathBuf,
    pub len: u64,
    pub archive: ZipArchive<fs::File>,
}

impl ArchiveWithPath {
    pub fn open(path: PathBuf) -> Result<Self, CommandError> {
        let f = fs::File::open(&path)
            .wrap_err_with(|| format!("failed to open zip input file path {:?}", &path))?;
        let len = f
            .metadata()
            .wrap_err("failed to extract file metadata")?
            .len();
        let archive = ZipArchive::new(f)
            .wrap_err_with(|| format!("failed to create zip archive from file {:?}", &path))?;
        Ok(Self { path, len, archive })
    }
}

fn format_entry_info(
    mut err: impl Write,
    entry_formatter: &CompiledFormatSpec<CompiledEntryDirective>,
    matcher: Option<&CompiledMatcher>,
    mut output_stream: impl Write,
    source: &mut impl IterateEntries,
) -> Result<(), CommandError> {
    if entry_formatter.is_empty() {
        writeln!(
            &mut err,
            "empty entry format, skipping reading from any entries"
        )
        .unwrap();
        return Ok(());
    }

    while let Some(entry) = source.next_entry()? {
        let data = EntryData::from_entry(&entry);
        if matcher.as_ref().is_some_and(|m| !m.matches(&data)) {
            writeln!(&mut err, "matcher ignored entry: {:?}", data.name).unwrap();
            continue;
        }
        entry_formatter.execute_format(data, &mut output_stream)?;
    }
    Ok(())
}

fn format_archive_info(
    mut err: impl Write,
    archive_formatter: &CompiledFormatSpec<CompiledArchiveDirective>,
    mut output_stream: impl Write,
    zip: ArchiveData,
) -> Result<(), CommandError> {
    if archive_formatter.is_empty() {
        writeln!(&mut err, "empty archive format, skipping archive overview").unwrap();
        return Ok(());
    }

    archive_formatter.execute_format(zip, &mut output_stream)?;
    Ok(())
}

pub fn execute_info(mut err: impl Write, args: Info) -> Result<(), CommandError> {
    let Info {
        format_spec,
        match_expr,
        input_spec: InputSpec {
            stdin_stream,
            zip_paths,
        },
    } = args;

    let matcher = match match_expr {
        None => None,
        Some(expr) => Some(CompiledMatcher::from_arg(expr)?),
    };
    let (archive_formatter, entry_formatter) = match format_spec {
        FormatSpec::Compact => todo!(),
        FormatSpec::Extended => todo!(),
        FormatSpec::Custom { overview, entry } => (
            CompiledFormatSpec::from_spec::<CompiledArchiveFormat>(overview)?,
            CompiledFormatSpec::from_spec::<CompiledEntryFormat>(entry)?,
        ),
    };
    let mut output_stream = io::stdout().lock();

    if stdin_stream {
        let mut stdin = StreamInput::new(io::stdin().lock());

        format_entry_info(
            &mut err,
            &entry_formatter,
            matcher.as_ref(),
            &mut output_stream,
            &mut stdin,
        )?;

        let (stdin, num_entries) = stdin.into_inner();
        /* NB: The read_zipfile_from_stream() method overruns the size of a single local header into
         * the CDE after reading the last input. There are unstable APIs to address this, but for
         * now just rely on that internal knowledge. See e.g. zip::read::stream on master or
         * zip::unstable::read in https://github.com/zip-rs/zip2/pull/233. */
        let cde_start = stdin.current_bytes_read() - 30;
        let (_stdin, stream_length) = stdin
            .exhaust()
            .wrap_err("failed to exhaust all of stdin after reading all zip entries")?;

        let data = ArchiveData {
            path: None,
            stream_length,
            num_entries,
            comment: None,
            first_entry_start: Some(0),
            central_directory_start: Some(cde_start),
        };
        format_archive_info(&mut err, &archive_formatter, &mut output_stream, data)?;
    }

    for p in zip_paths.into_iter() {
        let mut zip = ArchiveWithPath::open(p)?;

        {
            let mut zip_entry_counter = ZipFileInput::new(&mut zip.archive);
            format_entry_info(
                &mut err,
                &entry_formatter,
                matcher.as_ref(),
                &mut output_stream,
                &mut zip_entry_counter,
            )?;
        }

        let data = ArchiveData::from_archive_with_path(&zip);
        format_archive_info(&mut err, &archive_formatter, &mut output_stream, data)?;
    }

    Ok(())
}
