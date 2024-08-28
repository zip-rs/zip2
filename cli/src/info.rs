use std::{
    borrow::Cow,
    collections::HashMap,
    convert::Infallible,
    fmt, fs,
    io::{self, Write},
    marker::PhantomData,
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
};

use zip::{
    read::{read_zipfile_from_stream, ZipArchive, ZipFile},
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

trait FormatValue {
    type Input<'a>;
    type Output<'a>: AsRef<str>;
    type E: fmt::Display;
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
                EntryKind::File => "f",
                EntryKind::Dir => "d",
                EntryKind::Symlink => "s",
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

static GENERATED_MODE_STRINGS: LazyLock<Mutex<HashMap<(Option<u32>, UnixModeFormat), Arc<str>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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

impl FormatValue for UnixModeValue {
    type Input<'a> = Option<u32>;
    type Output<'a> = Arc<str>;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        Ok(Arc::clone(
            GENERATED_MODE_STRINGS
                .lock()
                .unwrap()
                .entry((input, self.0))
                .or_insert_with(|| {
                    let x = input.unwrap_or(0);
                    Arc::from(match self.0 {
                        UnixModeFormat::Octal => format!("{x:o}"),
                        UnixModeFormat::Pretty => {
                            String::from_utf8(Self::pretty_format_mode_bits(x).to_vec()).unwrap()
                        }
                    })
                }),
        ))
    }
}

#[derive(Copy, Clone)]
struct ByteSizeValue(ByteSizeFormat);

static ZERO_SIZE: &'static str = "0";

impl FormatValue for ByteSizeValue {
    type Input<'a> = u64;
    type Output<'a> = Cow<'static, str>;
    type E = Infallible;
    fn format_value<'a>(&self, input: Self::Input<'a>) -> Result<Self::Output<'a>, Self::E> {
        if input == 0 {
            return Ok(Cow::Borrowed(ZERO_SIZE));
        }
        Ok(Cow::Owned(match self.0 {
            ByteSizeFormat::FullDecimal => format!("{}", input),
            ByteSizeFormat::HumanAbbreviated => todo!("human abbreviated byte sizes"),
        }))
    }
}

struct ArchiveWithPath {
    pub path: PathBuf,
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
    type Data<'a> = EntryData<'a>;
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
    type Data<'a> = EntryData<'a>;
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
    type Data<'a> = EntryData<'a>;
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
    type Data<'a> = EntryData<'a>;
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
    type Data<'a> = EntryData<'a>;
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

trait ComponentFormatter {
    type Data<'a>;

    fn write_component<'a>(
        &self,
        data: Self::Data<'a>,
        out: &mut dyn Write,
    ) -> Result<usize, CommandError>;
}

impl<FD> ComponentFormatter for FD
where
    FD: FormatDirective,
{
    type Data<'a> = <FD as FormatDirective>::Data<'a>;

    fn write_component<'a>(
        &self,
        data: Self::Data<'a>,
        out: &mut dyn Write,
    ) -> Result<usize, CommandError> {
        let output = self
            .format_field(data)
            .map_err(|e| CommandError::InvalidData(format!("error formatting field: {e}")))?;
        let output: &str = output.as_ref();
        let n = output.len();
        out.write_all(output.as_bytes())
            .wrap_err_with(|| format!("failed to write output to stream: {output:?}"))?;
        Ok(n)
    }
}

trait EntryComponentFormatter {
    fn write_entry_component<'a>(
        &self,
        data: EntryData<'a>,
        out: &mut dyn Write,
    ) -> Result<usize, CommandError>;
}

impl<CF> EntryComponentFormatter for CF
where
    CF: for<'a> ComponentFormatter<Data<'a> = EntryData<'a>>,
{
    fn write_entry_component<'a>(
        &self,
        data: EntryData<'a>,
        out: &mut dyn Write,
    ) -> Result<usize, CommandError> {
        self.write_component(data, out)
    }
}

enum CompiledEntryFormatComponent {
    Directive(Box<dyn EntryComponentFormatter>),
    EscapedPercent,
    EscapedNewline,
    Literal(String),
}

impl CompiledEntryFormatComponent {
    fn compile_directive(
        spec: EntryFormatDirective,
    ) -> Result<Box<dyn EntryComponentFormatter>, CommandError> {
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
            EntryFormatComponent::Literal(lit) => Ok(Self::Literal(lit)),
        }
    }

    pub fn write_component<'a>(
        &self,
        data: EntryData<'a>,
        mut out: impl Write,
    ) -> Result<usize, CommandError> {
        match self {
            Self::Directive(directive) => directive.write_entry_component(data, &mut out),
            Self::EscapedPercent => out
                .write_all(b"%")
                .wrap_err("failed to write escaped % to output")
                .map(|()| 1),
            Self::EscapedNewline => out
                .write_all(b"\n")
                .wrap_err("failed to write escaped newline to output")
                .map(|()| 1),
            Self::Literal(lit) => out
                .write_all(lit.as_bytes())
                .wrap_err_with(|| format!("failed to write literal {lit:?} to output"))
                .map(|()| lit.len()),
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

    pub fn write_entry<'a>(
        &self,
        data: EntryData<'a>,
        mut out: impl Write,
    ) -> Result<usize, CommandError> {
        let mut written: usize = 0;
        for c in self.components.iter() {
            written += c.write_component(data, &mut out)?;
        }
        Ok(written)
    }
}

pub fn execute_info(err: impl Write, args: Info) -> Result<(), CommandError> {
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
        while let Some(entry) =
            read_zipfile_from_stream(&mut stdin).wrap_err("error reading zip entry from stdin")?
        {
            let data = EntryData::from_entry(&entry);
            entry_formatter.write_entry(data, &mut output_stream)?;
        }
    }
    for p in zip_paths.into_iter() {
        let mut zip = ArchiveWithPath::open(p.clone())?;
        for i in 0..zip.archive.len() {
            let entry = zip
                .archive
                .by_index(i)
                .wrap_err_with(|| format!("failed to read entry {i} from zip at {p:?}"))?;
            let data = EntryData::from_entry(&entry);
            entry_formatter.write_entry(data, &mut output_stream)?;
        }
    }

    Ok(())
}
