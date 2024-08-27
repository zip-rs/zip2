use super::{
    extract::{Extract, InputSpec, MatchExpression},
    ArgParseError, CommandFormat,
};

use std::{collections::VecDeque, ffi::OsString};

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
        "[-h|--help] [FORMAT-SPEC] [--expr MATCH-EXPR --expr] [--stdin] [--] [ZIP-PATH]...";

    fn generate_help() -> String {
        format!(
            r#"
  -h, --help	Print help

# Format specs:
???

{}

{}
{}
"#,
            Extract::generate_match_expr_help_text(),
            Extract::generate_pattern_selector_help_text(true),
            Extract::INPUT_HELP_TEXT,
        )
    }

    fn parse_argv(mut argv: VecDeque<OsString>) -> Result<Self, ArgParseError> {
        while let Some(arg) = argv.pop_front() {
            match arg.as_encoded_bytes() {
                b"-h" | b"--help" => {
                    let help_text = Self::generate_full_help_text();
                    return Err(ArgParseError::StdoutMessage(help_text));
                }
                _ => todo!(),
            }
        }
        todo!()
    }
}

impl crate::driver::ExecuteCommand for Info {
    fn execute(self, err: impl std::io::Write) -> Result<(), crate::CommandError> {
        todo!()
    }
}
