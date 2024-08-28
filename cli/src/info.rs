use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
};

use zip::read::{read_zipfile_from_stream, ZipArchive};

use crate::{
    args::{extract::InputSpec, info::*},
    extract::{
        matcher::{CompiledMatcher, EntryMatcher},
        receiver::EntryData,
    },
    CommandError, WrapCommandErr,
};

mod directives;
mod formats;
use directives::{compiled::CompiledFormatSpec, entry::compiled::CompiledEntryFormat};

struct ArchiveWithPath {
    pub path: PathBuf,
    /* TODO: Debug impl for ZipArchive? what about ZipFile? */
    pub archive: ZipArchive<fs::File>,
}

impl ArchiveWithPath {
    pub fn open(path: PathBuf) -> Result<Self, CommandError> {
        let f = fs::File::open(&path)
            .wrap_err_with(|| format!("failed to open zip input file path {:?}", &path))?;
        let archive = ZipArchive::new(f)
            .wrap_err_with(|| format!("failed to create zip archive from file {:?}", &path))?;
        Ok(Self { path, archive })
    }
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
            (),
            CompiledFormatSpec::from_spec::<CompiledEntryFormat>(entry)?,
        ),
    };
    let mut output_stream = io::stdout().lock();

    if stdin_stream {
        let mut stdin = io::stdin().lock();
        if entry_formatter.is_empty() {
            writeln!(&mut err, "empty entry format, skipping stdin entries").unwrap();
        } else {
            while let Some(entry) = read_zipfile_from_stream(&mut stdin)
                .wrap_err("error reading zip entry from stdin")?
            {
                let data = EntryData::from_entry(&entry);
                if matcher.as_ref().is_some_and(|m| !m.matches(&data)) {
                    continue;
                }
                entry_formatter.execute_format(data, &mut output_stream)?;
            }
        }
        writeln!(
            &mut err,
            "stdin currently cannot provide archive format info"
        )
        .unwrap();
    }
    for p in zip_paths.into_iter() {
        let mut zip = ArchiveWithPath::open(p.clone())?;
        if entry_formatter.is_empty() {
            writeln!(
                &mut err,
                "empty entry format, skipping entries for file {p:?}"
            )
            .unwrap();
        } else {
            for i in 0..zip.archive.len() {
                let entry = zip
                    .archive
                    .by_index(i)
                    .wrap_err_with(|| format!("failed to read entry {i} from zip at {p:?}"))?;
                let data = EntryData::from_entry(&entry);
                if matcher.as_ref().is_some_and(|m| !m.matches(&data)) {
                    continue;
                }
                entry_formatter.execute_format(data, &mut output_stream)?;
            }
        }
    }

    Ok(())
}
