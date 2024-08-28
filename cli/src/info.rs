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
use directives::{
    entry::{
        CompressionMethodField, EntryNameField, FileTypeField, UncompressedSizeField, UnixModeField,
    },
    DirectiveFormatter,
};
use formats::{ByteSizeValue, CompressionMethodValue, FileTypeValue, NameString, UnixModeValue};

trait EntryDirectiveFormatter {
    fn write_entry_directive<'a>(
        &self,
        data: &EntryData<'a>,
        out: &mut dyn Write,
    ) -> Result<(), CommandError>;
}

impl<CF> EntryDirectiveFormatter for CF
where
    CF: for<'a> DirectiveFormatter<Data<'a> = &'a EntryData<'a>>,
{
    fn write_entry_directive<'a>(
        &self,
        data: &EntryData<'a>,
        out: &mut dyn Write,
    ) -> Result<(), CommandError> {
        self.write_directive(data, out)
    }
}

enum CompiledEntryFormatComponent {
    Directive(Box<dyn EntryDirectiveFormatter>),
    EscapedPercent,
    EscapedNewline,
    EscapedTab,
    Literal(String),
}

impl CompiledEntryFormatComponent {
    fn compile_directive(
        spec: EntryFormatDirective,
    ) -> Result<Box<dyn EntryDirectiveFormatter>, CommandError> {
        Ok(match spec {
            EntryFormatDirective::Name => Box::new(EntryNameField(NameString)),
            EntryFormatDirective::FileType(f) => Box::new(FileTypeField(FileTypeValue(f))),
            EntryFormatDirective::UncompressedSize(f) => {
                Box::new(UncompressedSizeField(ByteSizeValue(f)))
            }
            EntryFormatDirective::UnixMode(f) => Box::new(UnixModeField(UnixModeValue(f))),
            EntryFormatDirective::CompressionMethod(f) => {
                Box::new(CompressionMethodField(CompressionMethodValue(f)))
            }
            _ => todo!(),
        })
    }

    pub fn from_spec(spec: EntryFormatComponent) -> Result<Self, CommandError> {
        match spec {
            EntryFormatComponent::Directive(directive) => {
                Ok(Self::Directive(Self::compile_directive(directive)?))
            }
            EntryFormatComponent::EscapedPercent => Ok(Self::EscapedPercent),
            EntryFormatComponent::EscapedNewline => Ok(Self::EscapedNewline),
            EntryFormatComponent::EscapedTab => Ok(Self::EscapedTab),
            EntryFormatComponent::Literal(lit) => Ok(Self::Literal(lit)),
        }
    }

    pub fn write_component<'a>(
        &self,
        data: &EntryData<'a>,
        mut out: impl Write,
    ) -> Result<(), CommandError> {
        match self {
            Self::Directive(directive) => directive.write_entry_directive(data, &mut out),
            Self::EscapedPercent => out
                .write_all(b"%")
                .wrap_err("failed to write escaped % to output"),
            Self::EscapedNewline => out
                .write_all(b"\n")
                .wrap_err("failed to write escaped newline to output"),
            Self::EscapedTab => out
                .write_all(b"\t")
                .wrap_err("failed to write escaped tab to output"),
            Self::Literal(lit) => out
                .write_all(lit.as_bytes())
                .wrap_err_with(|| format!("failed to write literal {lit:?} to output")),
        }
    }
}

struct CompiledEntryFormatter {
    components: Vec<CompiledEntryFormatComponent>,
}

impl CompiledEntryFormatter {
    pub fn from_spec(spec: EntryFormatSpec) -> Result<Self, CommandError> {
        let EntryFormatSpec { components } = spec;
        let components: Vec<_> = components
            .into_iter()
            .map(CompiledEntryFormatComponent::from_spec)
            .collect::<Result<_, _>>()?;
        Ok(Self { components })
    }

    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    pub fn write_entry<'a>(
        &self,
        data: EntryData<'a>,
        mut out: impl Write,
    ) -> Result<(), CommandError> {
        for c in self.components.iter() {
            c.write_component(&data, &mut out)?;
        }
        Ok(())
    }
}

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
        FormatSpec::Custom { overview, entry } => ((), CompiledEntryFormatter::from_spec(entry)?),
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
                entry_formatter.write_entry(data, &mut output_stream)?;
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
                entry_formatter.write_entry(data, &mut output_stream)?;
            }
        }
    }

    Ok(())
}
