use super::{ArgParseError, CommandFormat};

use std::{collections::VecDeque, ffi::OsString, num::ParseIntError, path::PathBuf};

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompressionMethodArg {
    Stored,
    Deflate, /* requires having zip/_deflate-any set to compile */
    #[cfg(feature = "deflate64")]
    Deflate64,
    #[cfg(feature = "bzip2")]
    Bzip2,
    #[cfg(feature = "zstd")]
    Zstd,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CompressionLevel(pub i64);

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnixPermissions(pub u32);

impl UnixPermissions {
    pub fn parse(s: &str) -> Result<Self, ParseIntError> {
        Ok(Self(u32::from_str_radix(s, 8)?))
    }
}

#[derive(Debug)]
pub enum CompressionArg {
    CompressionMethod(CompressionMethodArg),
    Level(CompressionLevel),
    UnixPermissions(UnixPermissions),
    LargeFile(bool),
    Name(String),
    Dir,
    Symlink,
    Immediate(OsString),
    FilePath(PathBuf),
    RecursiveDirPath(PathBuf),
}

#[derive(Debug)]
pub enum OutputType {
    Stdout { allow_tty: bool },
    File { path: PathBuf, append: bool },
}

#[derive(Debug)]
pub struct Compress {
    pub output: OutputType,
    pub archive_comment: Option<OsString>,
    pub args: Vec<CompressionArg>,
    pub positional_paths: Vec<PathBuf>,
}

impl Compress {
    #[cfg(feature = "deflate64")]
    const DEFLATE64_HELP_LINE: &'static str = "          - deflate64:\twith deflate64\n";
    #[cfg(not(feature = "deflate64"))]
    const DEFLATE64_HELP_LINE: &'static str = "";

    #[cfg(feature = "bzip2")]
    const BZIP2_HELP_LINE: &'static str = "          - bzip2:\twith bzip2\n";
    #[cfg(not(feature = "bzip2"))]
    const BZIP2_HELP_LINE: &'static str = "";

    #[cfg(feature = "zstd")]
    const ZSTD_HELP_LINE: &'static str = "          - zstd:\twith zstd\n";
    #[cfg(not(feature = "zstd"))]
    const ZSTD_HELP_LINE: &'static str = "";
}

/* TODO: add support for entry and file comments! */
impl CommandFormat for Compress {
    const COMMAND_NAME: &'static str = "compress";
    const COMMAND_TABS: &'static str = "\t";
    const COMMAND_DESCRIPTION: &'static str =
        "Generate an archive from data in argument strings or read from the filesystem.";

    const USAGE_LINE: &'static str =
        "[-h|--help] [OUTPUT-FLAGS] [--archive-comment <comment>] [ENTRY]... [--] [PATH]...";

    fn generate_help() -> String {
        format!(
            r#"
  -h, --help	Print help

Output flags:
Where and how to write the generated zip archive.

  -o, --output-file <file>
          Output zip file path to write.
          The output file is truncated if it already exists, unless --append is
          provided. If not provided, output is written to stdout.

      --append
          If an output path is provided with -o, open it as an existing zip
          archive and append to it. If the output path does not already exist,
          no error is produced, and a new zip file is created at the given path.

      --stdout
          Allow writing output to stdout even if stdout is a tty.

Global flags:
These flags describe information set for the entire produced archive.

      --archive-comment <comment>
          If provided, this will set the archive's comment field to the
          specified bytes. This does not need to be valid unicode.

Entries:
After output flags are provided, the rest of the command line is
attributes and entry data. Attributes modify later entries.

Sticky attributes:
These flags apply to everything that comes after them until reset by another
instance of the same attribute. Sticky attributes continue to apply to
positional arguments received after processing all flags.

  -c, --compression-method <method-name>
          Which compression technique to use.
          Defaults to deflate if not specified.

          Possible values:
          - stored:	uncompressed
          - deflate:	with deflate (default)
{}{}{}
  -l, --compression-level <level>
          How much compression to perform, from 0..=24.
          The accepted range of values differs for each technique.

  -m, --mode <mode>
          Unix permissions to apply to the file, in octal (like chmod).

      --large-file [true|false]
          Whether to enable large file support.
          This may take up more space for records, but allows files over 32 bits
          in length to be written, up to 64 bit sizes.
          File arguments over 32 bits in length (either provided explicitly or
          encountered when traversing a recursive directory) will have this flag
          set automatically, without affecting the sticky value for
          later options.
          Therefore, this option likely never has to be set explicitly by
          the user.

Non-sticky attributes:
These flags only apply to the next entry after them, and may not be repeated.

  -n, --name <name>
          The name to apply to the entry. This must be UTF-8 encoded.

  -s, --symlink
          Make the next entry into a symlink entry.
          A symlink entry may be immediate with -i, or it may copy the target
          from an existing symlink with -f.

Entry data:
Each of these flags creates an entry in the output zip archive.

  -d, --dir
          Create a directory entry.
          A name must be provided beforehand with -n.

  -i, --immediate <data>
          Write an entry containing the data in the argument, which need not be
          UTF-8 encoded but will exit early upon encountering any null bytes.
          A name must be provided beforehand with -n.

  -f, --file <path>
          Write an entry with the contents of this file path.
          A name may be provided beforehand with -n, otherwise the name will be
          inferred from relativizing the given path to the working directory.
          Note that sockets are currently not supported and will produce an
          error. Providing a path to a directory will produce an error.

          If -s was specified beforehand, the path will be read as a symlink,
          which will produce an error if the path does not point to a symbolic
          link. If -s was not specified beforehand and a symlink path was
          provided, then the symbolic link will be interpreted as if it was
          a file with the contents of the symlink target, but with its name
          corresponding to the symlink path (unless overridden with -n).

  -r, --recursive-dir <dir>
          Write all the recursive contents of this directory path.
          A name may be provided beforehand with -n, which will be used as the
          prefix for all recursive contents of this directory. Otherwise, the
          name will be inferred from relativizing the given path to the
          working directory.

          -s is not allowed before this argument. If a path to a symbolic link
          is provided, it will be treated as if it pointed to a directory with
          the recursive contents of the target directory, but with its name
          corresponding to the symlink path (unless overridden with -n).
          Providing a symlink path which points to a file will produce an error.

Positional entries:
  [PATH]...
          Write the file or recursive directory contents, relativizing the path.
          If the given path points to a file, then a single file entry will
          be written.
          If the given path is a symlink, then a single symlink entry will
          be written.
          If the given path refers to a directory, then the recursive contents
          will be written, reproducing files and symlinks.
          Socket paths will produce an error.
"#,
            Self::DEFLATE64_HELP_LINE,
            Self::BZIP2_HELP_LINE,
            Self::ZSTD_HELP_LINE,
        )
    }

    fn parse_argv(mut argv: VecDeque<OsString>) -> Result<Self, ArgParseError> {
        let mut allow_stdout: bool = false;
        let mut append_to_output_path: bool = false;
        let mut output_path: Option<PathBuf> = None;
        let mut archive_comment: Option<OsString> = None;
        let mut args: Vec<CompressionArg> = Vec::new();
        let mut positional_paths: Vec<PathBuf> = Vec::new();

        while let Some(arg) = argv.pop_front() {
            match arg.as_encoded_bytes() {
                b"-h" | b"--help" => {
                    let help_text = Self::generate_full_help_text();
                    return Err(ArgParseError::StdoutMessage(help_text));
                }

                /* Output flags */
                b"--stdout" => {
                    if let Some(output_path) = output_path.take() {
                        return Err(Self::exit_arg_invalid(&format!(
                            "--stdout provided along with output file {output_path:?}"
                        )));
                    } else if append_to_output_path {
                        return Err(Self::exit_arg_invalid(
                            "--stdout provided along with --append",
                        ));
                    } else if !args.is_empty() || !positional_paths.is_empty() {
                        return Err(Self::exit_arg_invalid("--stdout provided after entries"));
                    } else if allow_stdout {
                        return Err(Self::exit_arg_invalid("--stdout provided twice"));
                    } else {
                        allow_stdout = true;
                    }
                }
                b"--append" => {
                    if append_to_output_path {
                        return Err(Self::exit_arg_invalid("--append provided twice"));
                    } else if !args.is_empty() || !positional_paths.is_empty() {
                        return Err(Self::exit_arg_invalid("--append provided after entries"));
                    } else if allow_stdout {
                        return Err(Self::exit_arg_invalid(
                            "--stdout provided along with --append",
                        ));
                    } else {
                        append_to_output_path = true;
                    }
                }
                b"-o" | b"--output-file" => {
                    let new_path = argv.pop_front().map(PathBuf::from).ok_or_else(|| {
                        Self::exit_arg_invalid("no argument provided for -o/--output-file")
                    })?;
                    if let Some(prev_path) = output_path.take() {
                        return Err(Self::exit_arg_invalid(&format!(
                            "--output-file provided twice: {prev_path:?} and {new_path:?}"
                        )));
                    } else if allow_stdout {
                        return Err(Self::exit_arg_invalid(
                            "--stdout provided along with output file",
                        ));
                    } else if !args.is_empty() || !positional_paths.is_empty() {
                        return Err(Self::exit_arg_invalid(
                            "-o/--output-file provided after entries",
                        ));
                    } else {
                        output_path = Some(new_path);
                    }
                }

                /* Global flags */
                b"--archive-comment" => {
                    let new_comment = argv.pop_front().ok_or_else(|| {
                        Self::exit_arg_invalid("no argument provided for --archive-comment")
                    })?;
                    if let Some(prev_comment) = archive_comment.take() {
                        return Err(Self::exit_arg_invalid(&format!(
                            "--archive-comment provided twice: {prev_comment:?} and {new_comment:?}"
                        )));
                    } else if !args.is_empty() || !positional_paths.is_empty() {
                        return Err(Self::exit_arg_invalid(
                            "--archive-comment provided after entries",
                        ));
                    } else {
                        archive_comment = Some(new_comment);
                    }
                }

                /* Attributes */
                b"-c" | b"--compression-method" => match argv.pop_front() {
                    None => {
                        return Err(Self::exit_arg_invalid(
                            "no argument provided for -c/--compression-method",
                        ))
                    }
                    Some(name) => match name.as_encoded_bytes() {
                        b"stored" => args.push(CompressionArg::CompressionMethod(
                            CompressionMethodArg::Stored,
                        )),
                        b"deflate" => args.push(CompressionArg::CompressionMethod(
                            CompressionMethodArg::Deflate,
                        )),
                        #[cfg(feature = "deflate64")]
                        b"deflate64" => args.push(CompressionArg::CompressionMethod(
                            CompressionMethodArg::Deflate64,
                        )),
                        #[cfg(feature = "bzip2")]
                        b"bzip2" => args.push(CompressionArg::CompressionMethod(
                            CompressionMethodArg::Bzip2,
                        )),
                        #[cfg(feature = "zstd")]
                        b"zstd" => args.push(CompressionArg::CompressionMethod(
                            CompressionMethodArg::Zstd,
                        )),
                        _ => {
                            return Err(Self::exit_arg_invalid(
                                "unrecognized compression method {name:?}",
                            ));
                        }
                    },
                },
                b"-l" | b"--compression-level" => match argv.pop_front() {
                    None => {
                        return Err(Self::exit_arg_invalid(
                            "no argument provided for -l/--compression-level",
                        ));
                    }
                    Some(level) => match level.into_string() {
                        Err(level) => {
                            return Err(Self::exit_arg_invalid(&format!(
                                "invalid unicode provided for compression level: {level:?}"
                            )));
                        }
                        Ok(level) => match level.parse::<i64>() {
                            Err(e) => {
                                return Err(Self::exit_arg_invalid(&format!(
                                    "failed to parse integer for compression level: {e}"
                                )));
                            }
                            Ok(level) => {
                                if (0..=24).contains(&level) {
                                    args.push(CompressionArg::Level(CompressionLevel(level)))
                                } else {
                                    return Err(Self::exit_arg_invalid(&format!(
                                        "compression level {level} was not between 0 and 24"
                                    )));
                                }
                            }
                        },
                    },
                },
                b"-m" | b"--mode" => match argv.pop_front() {
                    None => {
                        return Err(Self::exit_arg_invalid("no argument provided for -m/--mode"));
                    }
                    Some(mode) => match mode.into_string() {
                        Err(mode) => {
                            return Err(Self::exit_arg_invalid(&format!(
                                "invalid unicode provided for mode: {mode:?}"
                            )));
                        }
                        Ok(mode) => match UnixPermissions::parse(&mode) {
                            Err(e) => {
                                return Err(Self::exit_arg_invalid(&format!(
                                    "failed to parse integer for mode: {e}"
                                )));
                            }
                            Ok(mode) => args.push(CompressionArg::UnixPermissions(mode)),
                        },
                    },
                },
                b"--large-file" => match argv.pop_front() {
                    None => {
                        return Err(Self::exit_arg_invalid(
                            "no argument provided for --large-file",
                        ));
                    }
                    Some(large_file) => match large_file.as_encoded_bytes() {
                        b"true" => args.push(CompressionArg::LargeFile(true)),
                        b"false" => args.push(CompressionArg::LargeFile(false)),
                        _ => {
                            return Err(Self::exit_arg_invalid(&format!(
                                "unrecognized value for --large-file: {large_file:?}"
                            )));
                        }
                    },
                },

                /* Data */
                b"-n" | b"--name" => match argv.pop_front() {
                    None => {
                        return Err(Self::exit_arg_invalid("no argument provided for -n/--name"))
                    }
                    Some(name) => match name.into_string() {
                        Err(name) => {
                            return Err(Self::exit_arg_invalid(&format!(
                                "invalid unicode provided for name: {name:?}"
                            )));
                        }
                        Ok(name) => args.push(CompressionArg::Name(name)),
                    },
                },
                b"-s" | b"--symlink" => args.push(CompressionArg::Symlink),
                b"-d" | b"--dir" => args.push(CompressionArg::Dir),
                b"-i" | b"--immediate" => match argv.pop_front() {
                    None => {
                        return Err(Self::exit_arg_invalid(
                            "no argument provided for -i/--immediate",
                        ));
                    }
                    Some(data) => args.push(CompressionArg::Immediate(data)),
                },
                b"-f" | b"--file" => match argv.pop_front() {
                    None => {
                        return Err(Self::exit_arg_invalid("no argument provided for -f/--file"));
                    }
                    Some(file) => args.push(CompressionArg::FilePath(file.into())),
                },
                b"-r" | b"--recursive-dir" => match argv.pop_front() {
                    None => {
                        return Err(Self::exit_arg_invalid(
                            "no argument provided for -r/--recursive-dir",
                        ));
                    }
                    Some(dir) => args.push(CompressionArg::RecursiveDirPath(dir.into())),
                },

                /* Transition to positional args */
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

        positional_paths.extend(argv.into_iter().map(|arg| arg.into()));

        let output = if let Some(path) = output_path {
            OutputType::File {
                path,
                append: append_to_output_path,
            }
        } else {
            OutputType::Stdout {
                allow_tty: allow_stdout,
            }
        };

        Ok(Self {
            output,
            archive_comment,
            args,
            positional_paths,
        })
    }
}

impl crate::driver::ExecuteCommand for Compress {
    fn execute(self, err: impl std::io::Write) -> Result<(), crate::CommandError> {
        crate::compress::execute_compress(err, self)
    }
}
