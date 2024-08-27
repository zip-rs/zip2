use super::{
    extract::{Extract, InputSpec, MatchExpression},
    ArgParseError, CommandFormat,
};

use std::{collections::VecDeque, ffi::OsString, fmt, path::PathBuf};

#[derive(Debug)]
struct ModifierParseError(pub String);

impl fmt::Display for ModifierParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", &self.0)
    }
}

#[derive(Debug)]
enum DirectiveParseError {
    Modifier(String, ModifierParseError),
    Unrecognized(String),
}

impl fmt::Display for DirectiveParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Modifier(d, e) => {
                write!(f, "unrecognized modifier in directive {d:?}: {e}")
            }
            Self::Unrecognized(d) => {
                write!(f, "unrecognized directive: {d:?}")
            }
        }
    }
}

#[derive(Debug)]
enum FormatParseError {
    Directive(DirectiveParseError),
    Search(String),
}

impl fmt::Display for FormatParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Directive(e) => {
                write!(f, "{e}")
            }
            Self::Search(e) => {
                write!(f, "error in parsing logic: {e}")
            }
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ByteSizeFormat {
    #[default]
    FullDecimal,
    HumanAbbreviated,
}

impl ByteSizeFormat {
    pub fn parse(s: &str) -> Result<Self, ModifierParseError> {
        match s {
            "" => Ok(Self::default()),
            ":decimal" => Ok(Self::FullDecimal),
            ":human" => Ok(Self::HumanAbbreviated),
            _ => Err(ModifierParseError(format!(
                "unrecognized byte size format: {s:?}"
            ))),
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OffsetFormat {
    Decimal,
    #[default]
    Hexadecimal,
}

impl OffsetFormat {
    pub fn parse(s: &str) -> Result<Self, ModifierParseError> {
        match s {
            "" => Ok(Self::default()),
            ":decimal" => Ok(Self::Decimal),
            ":hex" => Ok(Self::Hexadecimal),
            _ => Err(ModifierParseError(format!(
                "unrecognized offset format: {s:?}"
            ))),
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BinaryStringFormat {
    #[default]
    PrintAsString,
    EscapeBinary,
    WriteBinaryContents,
}

impl BinaryStringFormat {
    pub fn parse(s: &str) -> Result<Self, ModifierParseError> {
        match s {
            "" => Ok(Self::default()),
            ":print" => Ok(Self::PrintAsString),
            ":escape" => Ok(Self::EscapeBinary),
            ":write" => Ok(Self::WriteBinaryContents),
            _ => Err(ModifierParseError(format!(
                "unrecognized string format: {s:?}"
            ))),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArchiveOverviewFormatDirective {
    ArchiveName,
    TotalSize(ByteSizeFormat),
    NumEntries,
    ArchiveComment(BinaryStringFormat),
    FirstEntryStart(OffsetFormat),
    CentralDirectoryStart(OffsetFormat),
}

impl ArchiveOverviewFormatDirective {
    pub fn parse(s: &str) -> Result<Self, DirectiveParseError> {
        match s {
            "name" => Ok(Self::ArchiveName),
            s if s.starts_with("size") => {
                let size_fmt = ByteSizeFormat::parse(&s["size".len()..])
                    .map_err(|e| DirectiveParseError::Modifier(s.to_string(), e))?;
                Ok(Self::TotalSize(size_fmt))
            }
            "num" => Ok(Self::NumEntries),
            s if s.starts_with("comment") => {
                let str_fmt = BinaryStringFormat::parse(&s["comment".len()..])
                    .map_err(|e| DirectiveParseError::Modifier(s.to_string(), e))?;
                Ok(Self::ArchiveComment(str_fmt))
            }
            s if s.starts_with("offset") => {
                let offset_fmt = OffsetFormat::parse(&s["offset".len()..])
                    .map_err(|e| DirectiveParseError::Modifier(s.to_string(), e))?;
                Ok(Self::FirstEntryStart(offset_fmt))
            }
            s if s.starts_with("cde-offset") => {
                let offset_fmt = OffsetFormat::parse(&s["cde-offset".len()..])
                    .map_err(|e| DirectiveParseError::Modifier(s.to_string(), e))?;
                Ok(Self::CentralDirectoryStart(offset_fmt))
            }
            _ => Err(DirectiveParseError::Unrecognized(s.to_string())),
        }
    }
}

trait ParseableFormat: Sized {
    type Component: Sized;
    const ESCAPED: Self::Component;
    fn make_literal(s: &str) -> Self::Component;
    fn parse_directive(s: &str) -> Result<Self::Component, DirectiveParseError>;
    fn from_components(components: Vec<Self::Component>) -> Self;

    fn parse_format(s: &str) -> Result<Self, FormatParseError> {
        let mut components: Vec<Self::Component> = Vec::new();
        let mut last_source_position: usize = 0;
        while let Some(pcnt_pos) = s[last_source_position..]
            .find('%')
            .map(|p| p + last_source_position)
        {
            /* Anything in between directives is a literal string. */
            if pcnt_pos > last_source_position {
                components.push(Self::make_literal(&s[last_source_position..pcnt_pos]));
                last_source_position = pcnt_pos;
            }
            let next_pcnt = s[(pcnt_pos + 1)..]
                .find('%')
                .map(|p| p + pcnt_pos + 1)
                .ok_or_else(|| {
                    FormatParseError::Search("% directive opened but not closed".to_string())
                })?;
            let directive_contents = &s[pcnt_pos..=next_pcnt];
            match directive_contents {
                /* An empty directive is a literal percent. */
                "%%" => {
                    components.push(Self::ESCAPED);
                }
                /* Otherwise, parse the space between percents. */
                d => {
                    let directive = Self::parse_directive(&d[1..(d.len() - 1)])
                        .map_err(FormatParseError::Directive)?;
                    components.push(directive);
                }
            }
            last_source_position += directive_contents.len();
        }
        if s.len() > last_source_position {
            components.push(Self::make_literal(&s[last_source_position..]));
        }
        Ok(Self::from_components(components))
    }
}

#[derive(Debug)]
pub enum ArchiveOverviewFormatComponent {
    Directive(ArchiveOverviewFormatDirective),
    EscapedPercent,
    Literal(String),
}

#[derive(Debug)]
pub struct ArchiveOverviewFormatSpec {
    pub components: Vec<ArchiveOverviewFormatComponent>,
}

impl ParseableFormat for ArchiveOverviewFormatSpec {
    type Component = ArchiveOverviewFormatComponent;
    const ESCAPED: Self::Component = ArchiveOverviewFormatComponent::EscapedPercent;
    fn make_literal(s: &str) -> Self::Component {
        ArchiveOverviewFormatComponent::Literal(s.to_string())
    }
    fn parse_directive(s: &str) -> Result<Self::Component, DirectiveParseError> {
        Ok(ArchiveOverviewFormatComponent::Directive(
            ArchiveOverviewFormatDirective::parse(s)?,
        ))
    }
    fn from_components(components: Vec<Self::Component>) -> Self {
        Self { components }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnixModeFormat {
    #[default]
    Octal,
    Pretty,
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TimestampFormat {
    UnixEpochMilliseconds,
    DateOnly,
    TimeOnly,
    #[default]
    DateAndTime,
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CompressionMethodFormat {
    Abbreviated,
    #[default]
    Full,
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BinaryNumericValueFormat {
    Decimal,
    #[default]
    Hexadecimal,
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileTypeFormat {
    Abbreviated,
    #[default]
    Full,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryFormatDirective {
    Name,
    FileType(FileTypeFormat),
    Comment(BinaryStringFormat),
    LocalHeaderStart(OffsetFormat),
    ContentStart(OffsetFormat),
    ContentEnd(OffsetFormat),
    CompressedSize(ByteSizeFormat),
    UncompressedSize(ByteSizeFormat),
    UnixMode(UnixModeFormat),
    CompressionMethod(CompressionMethodFormat),
    CrcValue(BinaryNumericValueFormat),
    Timestamp(TimestampFormat),
}

#[derive(Debug)]
pub enum EntryFormatComponent {
    Directive(EntryFormatDirective),
    Literal(String),
}

#[derive(Debug)]
pub struct EntryFormatSpec {
    components: Vec<EntryFormatComponent>,
}

impl EntryFormatSpec {
    pub fn parse(s: &str) -> Result<Self, ArgParseError> {
        todo!()
    }
}

#[derive(Debug, Default)]
pub enum FormatSpec {
    #[default]
    Compact,
    Extended,
    Custom {
        overview: ArchiveOverviewFormatSpec,
        entry: EntryFormatSpec,
    },
}

impl FormatSpec {
    pub fn parse_format_strings(
        archive_format: String,
        entry_format: String,
    ) -> Result<Self, ArgParseError> {
        let overview = ArchiveOverviewFormatSpec::parse_format(&archive_format).map_err(|e| {
            Info::exit_arg_invalid(&format!(
                "failed to parse archive format string {archive_format:?}: {e}"
            ))
        })?;
        dbg!(&overview);
        let entry = EntryFormatSpec::parse(&entry_format)?;
        Ok(Self::Custom { overview, entry })
    }
}

#[derive(Debug)]
pub struct Info {
    pub format_spec: FormatSpec,
    pub match_expr: Option<MatchExpression>,
    pub input_spec: InputSpec,
}

impl CommandFormat for Info {
    const COMMAND_NAME: &'static str = "info";
    const COMMAND_TABS: &'static str = "\t\t";
    const COMMAND_DESCRIPTION: &'static str =
        "Print info about archive contents and individual entries.";

    const USAGE_LINE: &'static str =
        "[-h|--help] [--extended|--format <archive-format> <entry-format>] [--expr MATCH-EXPR --expr] [--stdin] [--] [ZIP-PATH]...";

    fn generate_help() -> String {
        format!(
            r#"
  -h, --help	Print help

By default, a compact representation of the metadata within the top-level
archive and individual entries is printed to stdout. This format, along with the
"extended" format from --extended, is not stable for processing by external
tools. For stable output, a custom format string should be provided with
--format.

Note that the contents of individual entries are not accessible with this
command, and should instead be extracted with the '{}' subcommand, which can
write entries to stdout or a given file path as well as extracted into an
output directory.

      --extended
          Print a verbose description of all top-level archive and individual
          entry fields.

      --format <archive-format> <entry-format>
          Print a custom description of the top-level archive and individual
          entry metadata.

          Both format specs must be provided, but empty strings are
          accepted. Explicit trailing newlines must be specified and will not be
          inserted automatically.

# Format specs:

{}

{}
{}
"#,
            Extract::COMMAND_NAME,
            Extract::generate_match_expr_help_text(),
            Extract::generate_pattern_selector_help_text(true),
            Extract::INPUT_HELP_TEXT,
        )
    }

    fn parse_argv(mut argv: VecDeque<OsString>) -> Result<Self, ArgParseError> {
        let mut format_spec: Option<FormatSpec> = None;
        let mut match_expr: Option<MatchExpression> = None;
        let mut stdin_flag = false;
        let mut positional_zips: Vec<PathBuf> = Vec::new();

        while let Some(arg) = argv.pop_front() {
            match arg.as_encoded_bytes() {
                b"-h" | b"--help" => {
                    let help_text = Self::generate_full_help_text();
                    return Err(ArgParseError::StdoutMessage(help_text));
                }

                /* Try parsing format specs. */
                b"--extended" => {
                    if let Some(prev_spec) = format_spec.take() {
                        return Err(Self::exit_arg_invalid(&format!(
                            "format spec already provided before --extended: {prev_spec:?}"
                        )));
                    }
                    format_spec = Some(FormatSpec::Extended);
                }
                b"--format" => {
                    if let Some(prev_spec) = format_spec.take() {
                        return Err(Self::exit_arg_invalid(&format!(
                            "format spec already provided before --format: {prev_spec:?}"
                        )));
                    }
                    let archive_format = argv
                        .pop_front()
                        .ok_or_else(|| {
                            Self::exit_arg_invalid("no <archive-format> arg provided to --format")
                        })?
                        .into_string()
                        .map_err(|fmt_arg| {
                            Self::exit_arg_invalid(&format!(
                                "invalid unicode provided to --format: {fmt_arg:?}"
                            ))
                        })?;
                    let entry_format = argv
                        .pop_front()
                        .ok_or_else(|| {
                            Self::exit_arg_invalid("no <entry-format> arg provided to --format")
                        })?
                        .into_string()
                        .map_err(|fmt_arg| {
                            Self::exit_arg_invalid(&format!(
                                "invalid unicode provided to --format: {fmt_arg:?}"
                            ))
                        })?;
                    format_spec = Some(FormatSpec::parse_format_strings(
                        archive_format,
                        entry_format,
                    )?);
                }

                /* Try parsing match specs! */
                b"--expr" => {
                    let new_expr = MatchExpression::parse_argv::<Self>(&mut argv)?;
                    if let Some(prev_expr) = match_expr.take() {
                        return Err(Self::exit_arg_invalid(&format!(
                            "multiple match expressions provided: {prev_expr:?} and {new_expr:?}"
                        )));
                    }
                    match_expr = Some(new_expr);
                }

                /* Transition to input args */
                b"--stdin" => {
                    stdin_flag = true;
                }
                b"--" => break,
                arg_bytes => {
                    if arg_bytes.starts_with(b"-") {
                        return Err(Self::exit_arg_invalid(&format!(
                            "unrecognized flag {arg:?}"
                        )));
                    } else {
                        argv.push_front(arg);
                        break;
                    }
                }
            }
        }

        positional_zips.extend(argv.into_iter().map(|arg| arg.into()));
        if !stdin_flag && positional_zips.is_empty() {
            return Err(Self::exit_arg_invalid(
                "no zip input files were provided, and --stdin was not provided",
            ));
        };
        let input_spec = InputSpec {
            stdin_stream: stdin_flag,
            zip_paths: positional_zips,
        };

        let format_spec = format_spec.unwrap_or_default();

        Ok(Self {
            format_spec,
            match_expr,
            input_spec,
        })
    }
}

impl crate::driver::ExecuteCommand for Info {
    fn execute(self, err: impl std::io::Write) -> Result<(), crate::CommandError> {
        todo!()
    }
}
