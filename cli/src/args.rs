use std::{collections::VecDeque, ffi::OsString, fmt, sync::OnceLock};

#[derive(Debug)]
pub enum ArgParseError {
    StdoutMessage(String),
    StderrMessage(String),
}

#[derive(Debug)]
pub struct ZipCli {
    pub verbose: bool,
    pub command: ZipCommand,
}

#[derive(Debug)]
enum SubcommandName {
    Compress,
    Info,
    Extract,
}

static PARSED_EXE_NAME: OnceLock<String> = OnceLock::new();

impl ZipCli {
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const DESCRIPTION: &'static str = env!("CARGO_PKG_DESCRIPTION");

    pub const INTERNAL_ERROR_EXIT_CODE: i32 = 3;
    pub const ARGV_PARSE_FAILED_EXIT_CODE: i32 = 2;
    pub const NON_FAILURE_EXIT_CODE: i32 = 0;

    pub fn binary_name() -> &'static str {
        PARSED_EXE_NAME.get().expect("binary name was not set yet")
    }

    fn generate_version_text() -> String {
        format!("{} {}\n", Self::binary_name(), Self::VERSION)
    }

    fn generate_usage_line() -> String {
        format!("Usage: {} [OPTIONS] <COMMAND>", Self::binary_name())
    }

    fn generate_full_help_text() -> String {
        format!(
            "\
{}

{}

Commands:
  {}{}{}
  {}{}{}
  {}{}{}

Options:
  -v, --verbose	Write information logs to stderr
  -h, --help	Print help
  -V, --version	Print version
",
            Self::DESCRIPTION,
            Self::generate_usage_line(),
            compress::Compress::COMMAND_NAME,
            compress::Compress::COMMAND_TABS,
            compress::Compress::COMMAND_DESCRIPTION,
            info::Info::COMMAND_NAME,
            info::Info::COMMAND_TABS,
            info::Info::COMMAND_DESCRIPTION,
            extract::Extract::COMMAND_NAME,
            extract::Extract::COMMAND_TABS,
            extract::Extract::COMMAND_DESCRIPTION,
        )
    }

    fn generate_brief_help_text(context: &str) -> String {
        format!(
            "\
error: {context}

{}

For more information, try '--help'.
",
            Self::generate_usage_line()
        )
    }

    fn parse_up_to_subcommand_name(
        argv: &mut VecDeque<OsString>,
    ) -> Result<(bool, SubcommandName), ArgParseError> {
        let mut verbose: bool = false;
        let mut subcommand_name: Option<SubcommandName> = None;
        while subcommand_name.is_none() {
            match argv.pop_front() {
                None => {
                    let help_text = Self::generate_full_help_text();
                    return Err(ArgParseError::StderrMessage(help_text));
                }
                Some(arg) => match arg.as_encoded_bytes() {
                    b"-v" | b"--verbose" => verbose = true,
                    b"-V" | b"--version" => {
                        let version_text = Self::generate_version_text();
                        return Err(ArgParseError::StdoutMessage(version_text));
                    }
                    b"-h" | b"--help" => {
                        let help_text = Self::generate_full_help_text();
                        return Err(ArgParseError::StdoutMessage(help_text));
                    }
                    b"compress" => subcommand_name = Some(SubcommandName::Compress),
                    b"info" => subcommand_name = Some(SubcommandName::Info),
                    b"extract" => subcommand_name = Some(SubcommandName::Extract),
                    arg_bytes => {
                        let context = if arg_bytes.starts_with(b"-") {
                            format!("unrecognized global flag {arg:?}")
                        } else {
                            format!("unrecognized subcommand name {arg:?}")
                        };
                        let help_text = Self::generate_brief_help_text(&context);
                        return Err(ArgParseError::StderrMessage(help_text));
                    }
                },
            }
        }
        Ok((verbose, subcommand_name.unwrap()))
    }

    pub fn parse_argv(argv: impl IntoIterator<Item = OsString>) -> Result<Self, ArgParseError> {
        let mut argv: VecDeque<OsString> = argv.into_iter().collect();
        let exe_name: String = argv
            .pop_front()
            .expect("exe name not on command line")
            .into_string()
            .expect("exe name not valid unicode");
        PARSED_EXE_NAME
            .set(exe_name)
            .expect("exe name already written");
        let (verbose, subcommand_name) = Self::parse_up_to_subcommand_name(&mut argv)?;
        let command = match subcommand_name {
            SubcommandName::Info => ZipCommand::Info(info::Info::parse_argv(argv)?),
            SubcommandName::Extract => ZipCommand::Extract(extract::Extract::parse_argv(argv)?),
            SubcommandName::Compress => ZipCommand::Compress(compress::Compress::parse_argv(argv)?),
        };
        Ok(Self { verbose, command })
    }
}

#[derive(Debug)]
pub enum ZipCommand {
    Compress(compress::Compress),
    Info(info::Info),
    Extract(extract::Extract),
}

pub trait CommandFormat: fmt::Debug {
    const COMMAND_NAME: &'static str;
    const COMMAND_TABS: &'static str;
    const COMMAND_DESCRIPTION: &'static str;

    const USAGE_LINE: &'static str;

    fn generate_usage_line() -> String {
        format!(
            "Usage: {} {} {}",
            ZipCli::binary_name(),
            Self::COMMAND_NAME,
            Self::USAGE_LINE,
        )
    }

    fn generate_help() -> String;

    fn generate_full_help_text() -> String {
        format!(
            "\
{}

{}
{}",
            Self::COMMAND_DESCRIPTION,
            Self::generate_usage_line(),
            Self::generate_help(),
        )
    }

    fn generate_brief_help_text(context: &str) -> String {
        format!(
            "\
error: {context}

{}
",
            Self::generate_usage_line()
        )
    }

    fn exit_arg_invalid(context: &str) -> ArgParseError {
        let message = Self::generate_brief_help_text(context);
        ArgParseError::StderrMessage(message)
    }

    fn parse_argv(argv: VecDeque<OsString>) -> Result<Self, ArgParseError>
    where
        Self: Sized;
}

pub mod compress;
pub mod extract;
pub mod info;