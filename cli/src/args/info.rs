use super::{
    extract::{Extract, InputSpec, MatchExpression},
    ArgParseError, CommandFormat,
};

use std::{collections::VecDeque, ffi::OsString, path::PathBuf};

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ByteSizeFormat {
    #[default]
    FullDecimal,
    HumanAbbreviated,
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OffsetFormat {
    Decimal,
    #[default]
    Hexadecimal,
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BinaryStringFormat {
    #[default]
    PrintAsString,
    WriteBinaryContents,
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

#[derive(Debug)]
pub enum ArchiveOverviewFormatComponent {
    Directive(ArchiveOverviewFormatDirective),
    Literal(String),
}

#[derive(Debug)]
pub struct ArchiveOverviewFormatSpec {
    components: Vec<ArchiveOverviewFormatComponent>,
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
        todo!()
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
                    let new_expr = MatchExpression::parse_argv(&mut argv)?;
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
