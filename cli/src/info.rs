use std::{
    fs,
    io::{self, Write},
    marker::PhantomData,
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

enum CompiledFormatComponent<F> {
    Directive(F),
    ContiguousLiteral(String),
}

impl<F> CompiledFormatComponent<F>
where
    F: DirectiveFormatter,
{
    pub fn write_component<'a>(
        &self,
        data: <F as DirectiveFormatter>::Data<'a>,
        mut out: impl Write,
    ) -> Result<(), CommandError> {
        match self {
            Self::Directive(d) => d.write_directive(data, &mut out),
            Self::ContiguousLiteral(lit) => out
                .write_all(lit.as_bytes())
                .wrap_err_with(|| format!("failed to write literal {lit:?} to output")),
        }
    }
}

struct CompiledFormatSpec<F> {
    pub components: Vec<CompiledFormatComponent<F>>,
}

impl<F> CompiledFormatSpec<F> {
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }
}

impl<F> CompiledFormatSpec<F>
where
    F: DirectiveFormatter,
{
    pub fn from_spec<CF>(
        spec: ParseableFormatSpec<<CF as CompiledFormat>::Spec>,
    ) -> Result<Self, CommandError>
    where
        CF: CompiledFormat<Fmt = F>,
    {
        let ParseableFormatSpec {
            components: spec_components,
        } = spec;

        let mut components: Vec<CompiledFormatComponent<F>> = Vec::new();
        for c in spec_components.into_iter() {
            match c {
                ParseableFormatComponent::Directive(d) => {
                    let d = CF::from_directive_spec(d)?;
                    components.push(CompiledFormatComponent::Directive(d));
                }
                ParseableFormatComponent::Escaped(s) => match components.last_mut() {
                    Some(CompiledFormatComponent::ContiguousLiteral(ref mut last_lit)) => {
                        last_lit.push_str(s);
                    }
                    _ => {
                        components.push(CompiledFormatComponent::ContiguousLiteral(s.to_string()));
                    }
                },
                ParseableFormatComponent::Literal(new_lit) => match components.last_mut() {
                    Some(CompiledFormatComponent::ContiguousLiteral(ref mut last_lit)) => {
                        last_lit.push_str(new_lit.as_str());
                    }
                    _ => {
                        components.push(CompiledFormatComponent::ContiguousLiteral(new_lit));
                    }
                },
            }
        }

        Ok(Self { components })
    }

    pub fn execute_format<'a>(
        &self,
        data: <F as DirectiveFormatter>::Data<'a>,
        mut out: impl Write,
    ) -> Result<(), CommandError>
    where
        <F as DirectiveFormatter>::Data<'a>: Clone,
    {
        for c in self.components.iter() {
            c.write_component(data.clone(), &mut out)?
        }
        Ok(())
    }
}

struct CompiledEntryDirective(Box<dyn EntryDirectiveFormatter>);

impl DirectiveFormatter for CompiledEntryDirective {
    type Data<'a> = EntryData<'a>;

    fn write_directive<'a>(
        &self,
        data: Self::Data<'a>,
        out: &mut dyn Write,
    ) -> Result<(), CommandError> {
        self.0.write_entry_directive(&data, out)
    }
}

trait CompiledFormat {
    type Spec: ParseableDirective;
    type Fmt: DirectiveFormatter;

    fn from_directive_spec(spec: Self::Spec) -> Result<Self::Fmt, CommandError>;
}

struct CompiledEntryFormat;

impl CompiledFormat for CompiledEntryFormat {
    type Spec = EntryFormatDirective;
    type Fmt = CompiledEntryDirective;

    fn from_directive_spec(
        spec: EntryFormatDirective,
    ) -> Result<CompiledEntryDirective, CommandError> {
        Ok(CompiledEntryDirective(match spec {
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
        }))
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
