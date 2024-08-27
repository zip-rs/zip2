use super::{
    extract::{InputSpec, MatchExpression},
    ArgParseError, CommandFormat,
};

use std::{collections::VecDeque, ffi::OsString};

#[derive(Debug)]
pub struct Info {
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
        r#"
  -h, --help	Print help

...
"#
        .to_string()
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
