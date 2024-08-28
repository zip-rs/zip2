use std::{
    convert::Infallible,
    fmt, fs,
    io::{self, Write},
    path::PathBuf,
};

use zip::{
    read::{read_zipfile_from_stream, ZipArchive},
    CompressionMethod,
};

use crate::{
    args::{extract::InputSpec, info::*},
    extract::{
        matcher::{CompiledMatcher, EntryMatcher},
        receiver::{EntryData, EntryKind},
    },
    CommandError, WrapCommandErr,
};

trait Writeable {
    fn write_to(&self, out: &mut dyn Write) -> Result<(), io::Error>;
}

impl<S> Writeable for S
where
    S: fmt::Display,
{
    fn write_to(&self, out: &mut dyn Write) -> Result<(), io::Error> {
        write!(out, "{}", self)
    }
}

trait FormatValue {
    type Input<'a>;
    type Output<'a>;
    type E;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E>;
}

#[derive(Copy, Clone)]
struct NameString;

impl FormatValue for NameString {
    type Input<'a> = &'a str;
    type Output<'a> = &'a str;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        Ok(input)
    }
}

#[derive(Copy, Clone)]
struct FileTypeValue(FileTypeFormat);

impl FormatValue for FileTypeValue {
    type Input<'a> = EntryKind;
    type Output<'a> = &'static str;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        Ok(match self.0 {
            FileTypeFormat::Full => match input {
                EntryKind::File => "file",
                EntryKind::Dir => "directory",
                EntryKind::Symlink => "symlink",
            },
            FileTypeFormat::Abbreviated => match input {
                EntryKind::File => "-",
                EntryKind::Dir => "d",
                EntryKind::Symlink => "l",
            },
        })
    }
}

#[derive(Copy, Clone)]
struct CompressionMethodValue(CompressionMethodFormat);

impl FormatValue for CompressionMethodValue {
    type Input<'a> = CompressionMethod;
    type Output<'a> = &'static str;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        Ok(match self.0 {
            CompressionMethodFormat::Full => match input {
                CompressionMethod::Stored => "stored",
                CompressionMethod::Deflated => "deflate",
                #[cfg(feature = "deflate64")]
                CompressionMethod::Deflate64 => "deflate64",
                #[cfg(feature = "bzip2")]
                CompressionMethod::Bzip2 => "bzip2",
                #[cfg(feature = "zstd")]
                CompressionMethod::Zstd => "zstd",
                #[cfg(feature = "lzma")]
                CompressionMethod::Lzma => "lzma",
                #[cfg(feature = "xz")]
                CompressionMethod::Xz => "xz",
                _ => "unknown",
            },
            CompressionMethodFormat::Abbreviated => match input {
                CompressionMethod::Stored => "stor",
                CompressionMethod::Deflated => "defl",
                #[cfg(feature = "deflate64")]
                CompressionMethod::Deflate64 => "df64",
                #[cfg(feature = "bzip2")]
                CompressionMethod::Bzip2 => "bz2",
                #[cfg(feature = "zstd")]
                CompressionMethod::Zstd => "zst",
                #[cfg(feature = "lzma")]
                CompressionMethod::Lzma => "lz",
                #[cfg(feature = "xz")]
                CompressionMethod::Xz => "xz",
                _ => "?",
            },
        })
    }
}

#[derive(Copy, Clone)]
struct UnixModeValue(UnixModeFormat);

impl UnixModeValue {
    const S_IRUSR: u32 = 256;
    const S_IWUSR: u32 = 128;
    const S_IXUSR: u32 = 64;

    const S_IRGRP: u32 = 32;
    const S_IWGRP: u32 = 16;
    const S_IXGRP: u32 = 8;

    const S_IROTH: u32 = 4;
    const S_IWOTH: u32 = 2;
    const S_IXOTH: u32 = 1;

    const UNKNOWN_MODE_BITS: [u8; 9] = [b'?'; 9];

    fn pretty_format_mode_bits(mode: u32) -> [u8; 9] {
        let mut ret = [b'-'; 9];

        if mode & Self::S_IRUSR == Self::S_IRUSR {
            ret[0] = b'r';
        }
        if mode & Self::S_IWUSR == Self::S_IWUSR {
            ret[1] = b'w';
        }
        if mode & Self::S_IXUSR == Self::S_IXUSR {
            ret[2] = b'x';
        }

        if mode & Self::S_IRGRP == Self::S_IRGRP {
            ret[3] = b'r';
        }
        if mode & Self::S_IWGRP == Self::S_IWGRP {
            ret[4] = b'w';
        }
        if mode & Self::S_IXGRP == Self::S_IXGRP {
            ret[5] = b'x';
        }

        if mode & Self::S_IROTH == Self::S_IROTH {
            ret[6] = b'r';
        }
        if mode & Self::S_IWOTH == Self::S_IWOTH {
            ret[7] = b'w';
        }
        if mode & Self::S_IXOTH == Self::S_IXOTH {
            ret[8] = b'x';
        }

        ret
    }
}

#[derive(Debug)]
enum ModeValueWriter {
    Octal(Option<u32>),
    Pretty([u8; 9]),
}

impl Writeable for ModeValueWriter {
    fn write_to(&self, out: &mut dyn Write) -> Result<(), io::Error> {
        match self {
            Self::Octal(mode) => match mode {
                Some(bits) => write!(out, "{:o}", bits),
                None => write!(out, "?"),
            },
            Self::Pretty(bits) => out.write_all(bits.as_ref()),
        }
    }
}

impl FormatValue for UnixModeValue {
    type Input<'a> = Option<u32>;
    type Output<'a> = ModeValueWriter;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        Ok(match self.0 {
            UnixModeFormat::Octal => ModeValueWriter::Octal(input),
            UnixModeFormat::Pretty => ModeValueWriter::Pretty(match input {
                Some(bits) => Self::pretty_format_mode_bits(bits),
                None => Self::UNKNOWN_MODE_BITS,
            }),
        })
    }
}

#[derive(Copy, Clone)]
struct ByteSizeValue(ByteSizeFormat);

#[derive(Debug)]
enum ByteSizeWriter {
    FullDecimal(u64),
}

impl fmt::Display for ByteSizeWriter {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::FullDecimal(n) => write!(f, "{}", n),
        }
    }
}

impl FormatValue for ByteSizeValue {
    type Input<'a> = u64;
    type Output<'a> = ByteSizeWriter;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        Ok(match self.0 {
            ByteSizeFormat::FullDecimal => ByteSizeWriter::FullDecimal(input),
            ByteSizeFormat::HumanAbbreviated => todo!("human abbreviated byte sizes"),
        })
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

trait FormatDirective {
    type Data<'a>;
    type FieldType: FormatValue;
    fn extract_field<'a>(
        &self,
        data: Self::Data<'a>,
    ) -> <Self::FieldType as FormatValue>::Input<'a>;
    fn value_formatter(&self) -> Self::FieldType;

    fn format_field<'a>(
        &self,
        data: Self::Data<'a>,
    ) -> Result<<Self::FieldType as FormatValue>::Output<'a>, <Self::FieldType as FormatValue>::E>
    {
        self.value_formatter()
            .format_value(self.extract_field(data))
    }
}

struct EntryNameField(NameString);

impl FormatDirective for EntryNameField {
    type Data<'a> = &'a EntryData<'a>;
    type FieldType = NameString;
    fn extract_field<'a>(
        &self,
        data: Self::Data<'a>,
    ) -> <Self::FieldType as FormatValue>::Input<'a> {
        data.name
    }
    fn value_formatter(&self) -> NameString {
        self.0
    }
}

struct FileTypeField(FileTypeValue);

impl FormatDirective for FileTypeField {
    type Data<'a> = &'a EntryData<'a>;
    type FieldType = FileTypeValue;
    fn extract_field<'a>(
        &self,
        data: Self::Data<'a>,
    ) -> <Self::FieldType as FormatValue>::Input<'a> {
        data.kind
    }
    fn value_formatter(&self) -> FileTypeValue {
        self.0
    }
}

struct CompressionMethodField(CompressionMethodValue);

impl FormatDirective for CompressionMethodField {
    type Data<'a> = &'a EntryData<'a>;
    type FieldType = CompressionMethodValue;
    fn extract_field<'a>(
        &self,
        data: Self::Data<'a>,
    ) -> <Self::FieldType as FormatValue>::Input<'a> {
        data.compression
    }
    fn value_formatter(&self) -> CompressionMethodValue {
        self.0
    }
}

struct UnixModeField(UnixModeValue);

impl FormatDirective for UnixModeField {
    type Data<'a> = &'a EntryData<'a>;
    type FieldType = UnixModeValue;
    fn extract_field<'a>(
        &self,
        data: Self::Data<'a>,
    ) -> <Self::FieldType as FormatValue>::Input<'a> {
        data.unix_mode
    }
    fn value_formatter(&self) -> UnixModeValue {
        self.0
    }
}

struct UncompressedSizeField(ByteSizeValue);

impl FormatDirective for UncompressedSizeField {
    type Data<'a> = &'a EntryData<'a>;
    type FieldType = ByteSizeValue;
    fn extract_field<'a>(
        &self,
        data: Self::Data<'a>,
    ) -> <Self::FieldType as FormatValue>::Input<'a> {
        data.size
    }
    fn value_formatter(&self) -> ByteSizeValue {
        self.0
    }
}

trait DirectiveFormatter {
    type Data<'a>;

    fn write_directive<'a>(
        &self,
        data: Self::Data<'a>,
        out: &mut dyn Write,
    ) -> Result<(), CommandError>;
}

impl<FD> DirectiveFormatter for FD
where
    FD: FormatDirective,
    for<'a> <<FD as FormatDirective>::FieldType as FormatValue>::Output<'a>: Writeable + fmt::Debug,
    <<FD as FormatDirective>::FieldType as FormatValue>::E: fmt::Display,
{
    type Data<'a> = <FD as FormatDirective>::Data<'a>;

    fn write_directive<'a>(
        &self,
        data: Self::Data<'a>,
        out: &mut dyn Write,
    ) -> Result<(), CommandError> {
        let output = self
            .format_field(data)
            .map_err(|e| CommandError::InvalidData(format!("error formatting field: {e}")))?;
        output
            .write_to(out)
            .wrap_err_with(|| format!("failed to write output to stream: {output:?}"))
    }
}

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
