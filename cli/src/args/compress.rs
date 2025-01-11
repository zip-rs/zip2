use super::{ArgParseError, CommandFormat, ComposedCommand};

use zip::{write::SimpleFileOptions, CompressionMethod};

use std::{collections::VecDeque, ffi::OsString, num::ParseIntError, path::PathBuf};

pub mod resource;
use super::resource::{ArgvResource, Resource};
use resource::{GlobalFlagsResource, ModSeqResource, OutputFlagsResource};

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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OutputType {
    Stdout { allow_tty: bool },
    File { path: PathBuf, append: bool },
}

impl Default for OutputType {
    fn default() -> Self {
        Self::Stdout { allow_tty: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GlobalFlags {
    pub archive_comment: Option<OsString>,
}

impl Default for GlobalFlags {
    fn default() -> Self {
        Self {
            archive_comment: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntrySpec {
    Dir {
        name: String,
    },
    Immediate {
        name: String,
        data: OsString,
        symlink_flag: bool,
    },
    File {
        name: Option<String>,
        path: PathBuf,
        symlink_flag: bool,
    },
    RecDir {
        name: Option<String>,
        path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModificationOperation {
    CreateEntry {
        options: SimpleFileOptions,
        spec: EntrySpec,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModificationSequence {
    pub operations: Vec<ModificationOperation>,
}

impl Default for ModificationSequence {
    fn default() -> Self {
        Self {
            operations: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct Compress {
    pub output: OutputType,
    pub global_flags: GlobalFlags,
    pub mod_seq: ModificationSequence,
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

/* TODO: add support for entry comments! */
/* TODO: add support for merging/transforming other zips!! */
impl CommandFormat for Compress {
    const COMMAND_NAME: &'static str = "compress";
    const COMMAND_TABS: &'static str = "\t";
    const COMMAND_DESCRIPTION: &'static str =
        "Generate an archive from data in argument strings or read from the filesystem.";

    const USAGE_LINE: &'static str =
        "[-h|--help] [OUTPUT-FLAGS] [GLOBAL-FLAGS] [ATTR|ENTRY-DATA]... [--] [ENTRY-PATH]...";

    fn generate_help() -> String {
        format!(
            r#"
  -h, --help	Print help

Output flags (OUTPUT-FLAGS): Where and how to write the generated zip archive.

If not specified, output is written to stdout.

OUTPUT-FLAGS = [--append] --output-file <file>
             = --stdout

  -o, --output-file <file>
          Output zip file path to write.

          The output file is truncated if it already exists, unless --append is
          provided.

      --append
          If an output path is provided with -o, open it as an existing zip
          archive and append to it.

          If the output path does not already exist, no error is produced, and
          a new zip file is created at the given path.

      --stdout
          Allow writing output to stdout even if stdout is a tty.

Global flags (GLOBAL-FLAGS): These flags describe information set for the entire produced archive.

GLOBAL-FLAGS = --archive-comment <comment>

      --archive-comment <comment>
          If provided, this will set the archive's comment field to the
          specified bytes. This does not need to be valid unicode.

Attributes (ATTR): Settings for entry metadata.

Attributes may be "sticky" or "non-sticky". Sticky attributes apply to
everything that comes after them, while non-sticky attributes only apply to the
next entry after them.

ATTR = STICKY
     = NON-STICKY

Sticky attributes (STICKY): Generic metadata.

These flags apply to everything that comes after them until reset by another
instance of the same attribute.

STICKY = --compression-method <method-name>
       = --compression-level <level>
       = --mode <mode>
       = --large-file <bool> # [true|false]

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
          TODO: how much???

  -m, --mode <mode>
          Unix permissions to apply to the file, in octal (like chmod).

      --large-file <bool> # [true|false]
          Whether to enable large file support.
          This may take up more space for records, but allows files over 32 bits
          in length to be written, up to 64 bit sizes.
          File arguments over 32 bits in length (either provided explicitly or
          encountered when traversing a recursive directory) will have this flag
          set automatically, without affecting the sticky value for
          later options.
          Therefore, this option likely never has to be set explicitly by
          the user.

Non-sticky attributes (NON-STICKY): Metadata for a single entry.

These flags only apply to the next entry after them, and may not be repeated.

NON-STICKY = --name <name>
           = --symlink

  -n, --name <name>
          The name to apply to the entry. This must be UTF-8 encoded.

  -s, --symlink
          Make the next entry into a symlink entry.

          A symlink entry may be immediate with -i, or it may copy the target
          from an existing symlink with -f.

Entry data (ENTRY-DATA): Create an entry in the output zip archive.

ENTRY-DATA = --dir
           = --immediate <data>
           = --file <path>
           = --recursive-dir <dir>

  -d, --dir
          Create a directory entry.
          A name must be provided beforehand with -n.

  -i, --immediate <data>
          Write an entry containing the data in the argument

          This data need not be UTF-8 encoded, but will exit early upon
          encountering any null bytes. A name must be provided beforehand with
          -n.

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

Positional entries (ENTRY-PATH): Paths which are converted into entries.

Any sticky attributes will continue to apply to entries specified via path,
while any non-sticky attributes not matched to an explicit ENTRY-DATA will produce
an error.

ENTRY-PATH = <path>

  <path>
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

    fn parse_argv(mut argv: VecDeque<OsString>) -> Result<Self, ArgParseError>
    where
        Self: Sized,
    {
        ComposedCommand::parse_composed_argv(argv)
    }
}

impl ComposedCommand for Compress {
    type ResourceArgs = (OutputFlagsResource, GlobalFlagsResource, ModSeqResource);
    fn get_resource_args() -> Self::ResourceArgs {
        (
            OutputFlagsResource::declare(()),
            GlobalFlagsResource::declare(()),
            ModSeqResource::declare(()),
        )
    }
    fn from_resource_args(
        args: Self::ResourceArgs,
        mut argv: VecDeque<OsString>,
    ) -> Result<Self, ArgParseError> {
        let (output, global_flags, mod_seq) = args;
        let output = output
            .parse_argv(&mut argv)
            .map_err(|e| Self::exit_arg_invalid(&format!("{e}")))?;
        let global_flags = global_flags
            .parse_argv(&mut argv)
            .map_err(|e| Self::exit_arg_invalid(&format!("{e}")))?;
        let mod_seq = mod_seq
            .parse_argv(&mut argv)
            .map_err(|e| Self::exit_arg_invalid(&format!("{e:?}")))?;

        Ok(Self {
            output,
            global_flags,
            mod_seq,
        })
    }
}

impl crate::driver::ExecuteCommand for Compress {
    fn execute(self, err: impl std::io::Write) -> Result<(), crate::CommandError> {
        crate::compress::execute_compress(err, self)
    }
}
