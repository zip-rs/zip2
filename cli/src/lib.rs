//! ???

#![cfg_attr(docsrs, feature(doc_auto_cfg))]

use std::{fs, io};

pub mod args;
pub mod compress;
pub mod extract;
pub mod info;
pub mod print;
pub mod schema;

pub enum ErrHandle<W> {
    Output(W),
    NoOutput,
}

impl<W> io::Write for ErrHandle<W>
where
    W: io::Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Output(w) => w.write(buf),
            Self::NoOutput => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Output(w) => w.flush(),
            Self::NoOutput => Ok(()),
        }
    }
}

pub enum OutputHandle {
    File(fs::File),
    InMem(io::Cursor<Vec<u8>>),
}

impl io::Read for OutputHandle {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::File(f) => f.read(buf),
            Self::InMem(c) => c.read(buf),
        }
    }
}

impl io::Write for OutputHandle {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::File(f) => f.write(buf),
            Self::InMem(c) => c.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::File(f) => f.flush(),
            Self::InMem(c) => c.flush(),
        }
    }
}

impl io::Seek for OutputHandle {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        match self {
            Self::File(f) => f.seek(pos),
            Self::InMem(c) => c.seek(pos),
        }
    }
}

#[derive(Debug)]
pub enum CommandError {
    InvalidArg(String),
    InvalidData(String),
    Io(String, io::Error),
    Zip(String, zip::result::ZipError),
}

pub trait WrapCommandErr<T>: Sized {
    fn wrap_err(self, context: &str) -> Result<T, CommandError> {
        self.wrap_err_with(|| context.to_string())
    }
    fn wrap_err_with(self, f: impl FnOnce() -> String) -> Result<T, CommandError>;
}

impl<T> WrapCommandErr<T> for Result<T, io::Error> {
    fn wrap_err_with(self, f: impl FnOnce() -> String) -> Result<T, CommandError> {
        self.map_err(|e| CommandError::Io(f(), e))
    }
}

impl<T> WrapCommandErr<T> for Result<T, zip::result::ZipError> {
    fn wrap_err_with(self, f: impl FnOnce() -> String) -> Result<T, CommandError> {
        self.map_err(|e| CommandError::Zip(f(), e))
    }
}

pub mod driver {
    use std::env;
    use std::io::{self, Write};
    use std::process;

    use super::args::{ArgParseError, CommandFormat, ZipCli, ZipCommand};
    use super::{CommandError, ErrHandle};

    pub trait ExecuteCommand: CommandFormat {
        fn execute(self, err: impl Write) -> Result<(), CommandError>;

        fn do_main(self, mut err: impl Write) -> !
        where
            Self: Sized,
        {
            writeln!(&mut err, "{} args: {:?}", Self::COMMAND_NAME, &self).unwrap();
            match self.execute(err) {
                Ok(()) => process::exit(ZipCli::NON_FAILURE_EXIT_CODE),
                Err(e) => match e {
                    CommandError::InvalidArg(msg) => {
                        let msg = Self::generate_brief_help_text(&msg);
                        let _ = io::stderr().write_all(msg.as_bytes());
                        process::exit(ZipCli::ARGV_PARSE_FAILED_EXIT_CODE);
                    }
                    CommandError::InvalidData(msg) => {
                        let msg = format!("error processing zip data: {msg}\n");
                        let _ = io::stderr().write_all(msg.as_bytes());
                        process::exit(ZipCli::ARGV_PARSE_FAILED_EXIT_CODE);
                    }
                    CommandError::Io(context, e) => {
                        let msg = format!("i/o error: {context}: {e}\n");
                        let _ = io::stderr().write_all(msg.as_bytes());
                        process::exit(ZipCli::INTERNAL_ERROR_EXIT_CODE);
                    }
                    CommandError::Zip(context, e) => {
                        let msg = format!("zip error: {context}: {e}\n");
                        let _ = io::stderr().write_all(msg.as_bytes());
                        process::exit(ZipCli::INTERNAL_ERROR_EXIT_CODE);
                    }
                },
            }
        }
    }

    pub fn main() {
        let ZipCli { verbose, command } = match ZipCli::parse_argv(env::args_os()) {
            Ok(cli) => cli,
            Err(e) => match e {
                ArgParseError::StdoutMessage(msg) => {
                    io::stdout()
                        .write_all(msg.as_bytes())
                        .expect("couldn't write message to stdout");
                    process::exit(ZipCli::NON_FAILURE_EXIT_CODE);
                }
                ArgParseError::StderrMessage(msg) => {
                    /* If we can't write anything to stderr, no use aborting, so just exit. */
                    let _ = io::stderr().write_all(msg.as_bytes());
                    process::exit(ZipCli::ARGV_PARSE_FAILED_EXIT_CODE);
                }
            },
        };
        let err = if verbose {
            ErrHandle::Output(io::stderr())
        } else {
            ErrHandle::NoOutput
        };

        match command {
            ZipCommand::Info(info) => info.do_main(err),
            ZipCommand::Extract(extract) => extract.do_main(err),
            ZipCommand::Compress(compress) => compress.do_main(err),
            /* TODO: ZipCommand::Crawl! */
        }
    }
}
