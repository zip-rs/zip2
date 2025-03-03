use super::*;
use crate::args::resource::*;

pub struct OutputFlagsResource;

impl ResourceValue for OutputType {}

impl Resource for OutputFlagsResource {
    /* const ID: &'static str = "OUTPUT-FLAGS"; */
    type Value = OutputType;
    type Args = ();
    fn declare(args: Self::Args) -> Self {
        Self
    }
}

pub struct GlobalFlagsResource;

impl ResourceValue for GlobalFlags {}

impl Resource for GlobalFlagsResource {
    /* const ID: &'static str = "GLOBAL-FLAGS"; */
    type Value = GlobalFlags;
    type Args = ();
    fn declare(args: Self::Args) -> Self {
        Self
    }
}

pub struct ModSeqResource;

impl ResourceValue for ModificationSequence {}

impl Resource for ModSeqResource {
    /* const ID: &'static str = "MOD-SEQ"; */
    type Value = ModificationSequence;
    type Args = ();
    fn declare(args: Self::Args) -> Self {
        Self
    }
}

pub mod argv {
    use super::*;

    use std::{collections::VecDeque, error, ffi::OsString, fmt, path::PathBuf};

    #[derive(Debug)]
    pub enum OutputTypeError {
        ArgWith(&'static str, String),
        ArgTwice(&'static str),
        NoValFor(&'static str),
        ValArgTwice {
            arg: &'static str,
            prev: String,
            new: String,
        },
    }

    impl fmt::Display for OutputTypeError {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            match self {
                Self::ArgWith(arg_name, other_entity) => {
                    write!(f, "{arg_name} is mutually exclusive with {other_entity}")
                }
                Self::ArgTwice(arg_name) => {
                    write!(f, "{arg_name} provided twice")
                }
                Self::NoValFor(arg_name) => {
                    write!(f, "no value provided for {arg_name}")
                }
                Self::ValArgTwice { arg, prev, new } => {
                    write!(
                        f,
                        "value provided twice for argument {arg}. prev was: {prev}, new was {new}"
                    )
                }
            }
        }
    }

    impl error::Error for OutputTypeError {}

    impl ArgvResource for OutputFlagsResource {
        /* fn print_help(&self) -> String { */
/*             r#" */
/* Output flags (OUTPUT-FLAGS): Where and how to write the generated zip archive. */

/* If not specified, output is written to stdout. */

/* OUTPUT-FLAGS = [--append] --output-file <file> */
/*              = --stdout */

/*   -o, --output-file <file> */
/*           Output zip file path to write. */

/*           The output file is truncated if it already exists, unless --append is */
/*           provided. */

/*       --append */
/*           If an output path is provided with -o, open it as an existing zip */
/*           archive and append to it. */

/*           If the output path does not already exist, no error is produced, and */
/*           a new zip file is created at the given path. */

/*       --stdout */
/*           Allow writing output to stdout even if stdout is a tty. */
/* "# */
/*         } */

        type ArgvParseError = OutputTypeError;
        fn parse_argv(
            &self,
            argv: &mut VecDeque<OsString>,
        ) -> Result<OutputType, Self::ArgvParseError> {
            let mut allow_stdout: bool = false;
            let mut append_to_output_path: bool = false;
            let mut output_path: Option<PathBuf> = None;

            while let Some(arg) = argv.pop_front() {
                match arg.as_encoded_bytes() {
                    b"--stdout" => {
                        if let Some(output_path) = output_path.take() {
                            return Err(OutputTypeError::ArgWith(
                                "--stdout",
                                format!("output file {output_path:?}"),
                            ));
                        }
                        if append_to_output_path {
                            return Err(OutputTypeError::ArgWith(
                                "--stdout",
                                "--append".to_string(),
                            ));
                        }
                        if allow_stdout {
                            return Err(OutputTypeError::ArgTwice("--stdout"));
                        }
                        allow_stdout = true;
                    }
                    b"--append" => {
                        if append_to_output_path {
                            return Err(OutputTypeError::ArgTwice("--append"));
                        }
                        if allow_stdout {
                            return Err(OutputTypeError::ArgWith(
                                "--append",
                                "--stdout".to_string(),
                            ));
                        }
                        append_to_output_path = true;
                    }
                    b"-o" | b"--output-file" => {
                        let new_path = argv
                            .pop_front()
                            .map(PathBuf::from)
                            .ok_or_else(|| OutputTypeError::NoValFor("-o/--output-file"))?;
                        if let Some(prev_path) = output_path.take() {
                            return Err(OutputTypeError::ValArgTwice {
                                arg: "-o/--output-file",
                                prev: format!("{prev_path:?}"),
                                new: format!("{new_path:?}"),
                            });
                        }
                        if allow_stdout {
                            return Err(OutputTypeError::ArgWith(
                                "--stdout",
                                "-o/--output-file".to_string(),
                            ));
                        }
                        output_path = Some(new_path);
                    }
                    _ => {
                        argv.push_front(arg);
                        break;
                    }
                }
            }

            Ok(if let Some(output_path) = output_path {
                OutputType::File {
                    path: output_path,
                    append: append_to_output_path,
                }
            } else {
                OutputType::Stdout {
                    allow_tty: allow_stdout,
                }
            })
        }
    }

    #[derive(Debug)]
    pub enum GlobalFlagsError {
        NoValFor(&'static str),
        ValArgTwice {
            arg: &'static str,
            prev: String,
            new: String,
        },
    }

    impl fmt::Display for GlobalFlagsError {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            match self {
                Self::NoValFor(arg_name) => {
                    write!(f, "no value provided for {arg_name}")
                }
                Self::ValArgTwice { arg, prev, new } => {
                    write!(
                        f,
                        "value provided twice for argument {arg}. prev was: {prev}, new was {new}"
                    )
                }
            }
        }
    }

    impl error::Error for GlobalFlagsError {}

    impl ArgvResource for GlobalFlagsResource {
        type ArgvParseError = GlobalFlagsError;
        fn parse_argv(
            &self,
            argv: &mut VecDeque<OsString>,
        ) -> Result<GlobalFlags, Self::ArgvParseError> {
            let mut archive_comment: Option<OsString> = None;

            while let Some(arg) = argv.pop_front() {
                match arg.as_encoded_bytes() {
                    b"--archive-comment" => {
                        let new_comment = argv
                            .pop_front()
                            .ok_or_else(|| GlobalFlagsError::NoValFor("--archive-comment"))?;
                        if let Some(prev_comment) = archive_comment.take() {
                            return Err(GlobalFlagsError::ValArgTwice {
                                arg: "--archive-comment",
                                prev: format!("{prev_comment:?}"),
                                new: format!("{new_comment:?}"),
                            });
                        }
                        archive_comment = Some(new_comment);
                    }
                    _ => {
                        argv.push_front(arg);
                        break;
                    }
                }
            }

            Ok(GlobalFlags { archive_comment })
        }
    }

    pub mod compression_args {
        use super::*;
        use crate::{schema::transformers::WrapperError, CommandError, WrapCommandErr};

        use zip::{unstable::path_to_string, write::SimpleFileOptions, CompressionMethod};

        use std::mem;

        #[derive(Debug)]
        pub enum ModificationSequenceError {
            NoValFor(&'static str),
            Unrecognized {
                context: &'static str,
                value: String,
            },
            ValidationFailed {
                codec: &'static str,
                context: &'static str,
                value: String,
            },
        }

        impl fmt::Display for ModificationSequenceError {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                match self {
                    Self::NoValFor(arg_name) => {
                        write!(f, "no value provided for {arg_name}")
                    }
                    Self::Unrecognized { context, value } => {
                        write!(f, "unrecognized {context}: {value}")
                    }
                    Self::ValidationFailed {
                        codec,
                        context,
                        value,
                    } => {
                        write!(f, "{codec} for {context}: {value}")
                    }
                }
            }
        }

        impl error::Error for ModificationSequenceError {}

        pub struct CompressionArgs {
            pub args: Vec<CompressionArg>,
            pub positional_paths: Vec<PathBuf>,
        }

        impl CompressionArgs {
            fn initial_options() -> SimpleFileOptions {
                SimpleFileOptions::default()
                    .compression_method(CompressionMethod::Deflated)
                    .large_file(false)
            }

            fn parse_compression_method(
                name: OsString,
            ) -> Result<CompressionArg, ModificationSequenceError> {
                Ok(match name.as_encoded_bytes() {
                    b"stored" => CompressionArg::CompressionMethod(CompressionMethodArg::Stored),
                    b"deflate" => CompressionArg::CompressionMethod(CompressionMethodArg::Deflate),
                    #[cfg(feature = "deflate64")]
                    b"deflate64" => {
                        CompressionArg::CompressionMethod(CompressionMethodArg::Deflate64)
                    }
                    #[cfg(feature = "bzip2")]
                    b"bzip2" => CompressionArg::CompressionMethod(CompressionMethodArg::Bzip2),
                    #[cfg(feature = "zstd")]
                    b"zstd" => CompressionArg::CompressionMethod(CompressionMethodArg::Zstd),
                    _ => {
                        return Err(ModificationSequenceError::Unrecognized {
                            context: "compression method",
                            value: format!("{name:?}"),
                        })
                    }
                })
            }

            fn parse_unicode(
                context: &'static str,
                arg: OsString,
            ) -> Result<String, ModificationSequenceError> {
                arg.into_string()
                    .map_err(|arg| ModificationSequenceError::ValidationFailed {
                        codec: "invalid unicode",
                        context,
                        value: format!("{arg:?}"),
                    })
            }

            fn parse_i64(
                context: &'static str,
                arg: String,
            ) -> Result<i64, ModificationSequenceError> {
                arg.parse::<i64>()
                    .map_err(|e| ModificationSequenceError::ValidationFailed {
                        codec: "failed to parse integer",
                        context,
                        value: format!("{e}"),
                    })
            }

            fn parse_compression_level(
                level: OsString,
            ) -> Result<CompressionArg, ModificationSequenceError> {
                let level = Self::parse_unicode("compression level", level)?;
                let level = Self::parse_i64("compression level", level)?;
                if (0..=24).contains(&level) {
                    Ok(CompressionArg::Level(CompressionLevel(level)))
                } else {
                    Err(ModificationSequenceError::ValidationFailed {
                        codec: "integer was not between 0 and 24",
                        context: "compression level",
                        value: format!("{level}"),
                    })
                }
            }

            fn parse_mode(mode: OsString) -> Result<CompressionArg, ModificationSequenceError> {
                let mode = Self::parse_unicode("mode", mode)?;
                let mode = UnixPermissions::parse(&mode).map_err(|e| {
                    ModificationSequenceError::ValidationFailed {
                        codec: "failed to parse octal integer",
                        context: "compression mode",
                        value: format!("{e}"),
                    }
                })?;
                Ok(CompressionArg::UnixPermissions(mode))
            }

            fn parse_large_file(
                large_file: OsString,
            ) -> Result<CompressionArg, ModificationSequenceError> {
                Ok(match large_file.as_encoded_bytes() {
                    b"true" => CompressionArg::LargeFile(true),
                    b"false" => CompressionArg::LargeFile(false),
                    _ => {
                        return Err(ModificationSequenceError::Unrecognized {
                            context: "value for --large-file",
                            value: format!("{large_file:?}"),
                        })
                    }
                })
            }

            pub fn parse_argv(
                argv: &mut VecDeque<OsString>,
            ) -> Result<Self, ModificationSequenceError> {
                let mut args: Vec<CompressionArg> = Vec::new();
                let mut positional_paths: Vec<PathBuf> = Vec::new();

                while let Some(arg) = argv.pop_front() {
                    let arg = match arg.as_encoded_bytes() {
                        /* Attributes */
                        b"-c" | b"--compression-method" => match argv.pop_front() {
                            None => Err(ModificationSequenceError::NoValFor(
                                "-c/--compression-method",
                            )),
                            Some(name) => Self::parse_compression_method(name),
                        },
                        b"-l" | b"--compression-level" => match argv.pop_front() {
                            None => Err(ModificationSequenceError::NoValFor(
                                "-l/--compression-level",
                            )),
                            Some(level) => Self::parse_compression_level(level),
                        },
                        b"-m" | b"--mode" => match argv.pop_front() {
                            None => Err(ModificationSequenceError::NoValFor("-m/--mode")),
                            Some(mode) => Self::parse_mode(mode),
                        },
                        b"--large-file" => match argv.pop_front() {
                            None => Err(ModificationSequenceError::NoValFor("--large-file")),
                            Some(large_file) => Self::parse_large_file(large_file),
                        },

                        /* Data */
                        b"-n" | b"--name" => match argv.pop_front() {
                            None => Err(ModificationSequenceError::NoValFor("-n/--name")),
                            Some(name) => {
                                Self::parse_unicode("name", name).map(CompressionArg::Name)
                            }
                        },
                        b"-s" | b"--symlink" => Ok(CompressionArg::Symlink),
                        b"-d" | b"--dir" => Ok(CompressionArg::Dir),
                        b"-i" | b"--immediate" => match argv.pop_front() {
                            None => Err(ModificationSequenceError::NoValFor("-i/--immediate")),
                            Some(data) => Ok(CompressionArg::Immediate(data)),
                        },
                        b"-f" | b"--file" => match argv.pop_front() {
                            None => Err(ModificationSequenceError::NoValFor("-f/--file")),
                            Some(file) => Ok(CompressionArg::FilePath(file.into())),
                        },
                        b"-r" | b"--recursive-dir" => match argv.pop_front() {
                            None => Err(ModificationSequenceError::NoValFor("-r/--recursive-dir")),
                            Some(dir) => Ok(CompressionArg::RecursiveDirPath(dir.into())),
                        },

                        /* Transition to positional args */
                        b"--" => break,
                        arg_bytes => {
                            if arg_bytes.starts_with(b"-") {
                                Err(ModificationSequenceError::Unrecognized {
                                    context: "flag",
                                    value: format!("{arg:?}"),
                                })
                            } else {
                                argv.push_front(arg);
                                break;
                            }
                        }
                    }?;
                    args.push(arg);
                }

                positional_paths.extend(mem::take(argv).into_iter().map(PathBuf::from));

                Ok(Self {
                    args,
                    positional_paths,
                })
            }

            fn interpret_entry_path(path: PathBuf) -> Result<EntrySpec, CommandError> {
                let file_type = std::fs::symlink_metadata(&path)
                    .wrap_err_with(|| format!("failed to read metadata from path {path:?}"))?
                    .file_type();
                Ok(if file_type.is_dir() {
                    EntrySpec::RecDir { name: None, path }
                } else {
                    EntrySpec::File {
                        name: None,
                        path,
                        symlink_flag: file_type.is_symlink(),
                    }
                })
            }

            pub fn build_mod_seq(
                self,
                /* mut err: impl Write, */
            ) -> Result<ModificationSequence, CommandError> {
                let Self {
                    args,
                    positional_paths,
                } = self;

                let mut operations: Vec<ModificationOperation> = Vec::new();

                let mut options = Self::initial_options();

                let mut last_name: Option<String> = None;
                let mut symlink_flag: bool = false;

                for arg in args.into_iter() {
                    match arg {
                        /* attributes: */
                        CompressionArg::CompressionMethod(method) => {
                            let method = match method {
                                CompressionMethodArg::Stored => CompressionMethod::Stored,
                                CompressionMethodArg::Deflate => CompressionMethod::Deflated,
                                #[cfg(feature = "deflate64")]
                                CompressionMethodArg::Deflate64 => CompressionMethod::Deflate64,
                                #[cfg(feature = "bzip2")]
                                CompressionMethodArg::Bzip2 => CompressionMethod::Bzip2,
                                #[cfg(feature = "zstd")]
                                CompressionMethodArg::Zstd => CompressionMethod::Zstd,
                            };
                            /* writeln!(err, "setting compression method {method:?}").unwrap(); */
                            options = options.compression_method(method);
                        }
                        CompressionArg::Level(CompressionLevel(level)) => {
                            /* writeln!(err, "setting compression level {level:?}").unwrap(); */
                            options = options.compression_level(Some(level));
                        }
                        CompressionArg::UnixPermissions(UnixPermissions(mode)) => {
                            /* writeln!(err, "setting file mode {mode:#o}").unwrap(); */
                            options = options.unix_permissions(mode);
                        }
                        CompressionArg::LargeFile(large_file) => {
                            /* writeln!(err, "setting large file flag to {large_file:?}").unwrap(); */
                            options = options.large_file(large_file);
                        }
                        CompressionArg::Name(name) => {
                            /* writeln!(err, "setting name of next entry to {name:?}").unwrap(); */
                            if let Some(last_name) = last_name {
                                return Err(CommandError::InvalidArg(format!(
                                    "got two names before an entry: {last_name} and {name}"
                                )));
                            }
                            last_name = Some(name);
                        }
                        CompressionArg::Symlink => {
                            /* writeln!(err, "setting symlink flag for next entry").unwrap(); */
                            if symlink_flag {
                                /* TODO: make this a warning? */
                                return Err(CommandError::InvalidArg(
                                    "symlink flag provided twice before entry".to_string(),
                                ));
                            }
                            symlink_flag = true;
                        }

                        /* new operations: */
                        CompressionArg::Dir => {
                            let last_name = last_name.take();
                            let symlink_flag = mem::replace(&mut symlink_flag, false);

                            /* writeln!(err, "writing dir entry").unwrap(); */
                            if symlink_flag {
                                return Err(CommandError::InvalidArg(
                                    "symlink flag provided before dir entry".to_string(),
                                ));
                            }
                            let name = last_name.ok_or_else(|| {
                                CommandError::InvalidArg(
                                    "no name provided before dir entry".to_string(),
                                )
                            })?;
                            operations.push(ModificationOperation::CreateEntry {
                                options,
                                spec: EntrySpec::Dir { name },
                            });
                        }
                        CompressionArg::Immediate(data) => {
                            let last_name = last_name.take();
                            let symlink_flag = mem::replace(&mut symlink_flag, false);

                            let name = last_name.ok_or_else(|| {
                                CommandError::InvalidArg(format!(
                                    "no name provided for immediate data {data:?}"
                                ))
                            })?;
                            operations.push(ModificationOperation::CreateEntry {
                                options,
                                spec: EntrySpec::Immediate {
                                    name,
                                    data,
                                    symlink_flag,
                                },
                            });
                        }
                        CompressionArg::FilePath(path) => {
                            let last_name = last_name.take();
                            let symlink_flag = mem::replace(&mut symlink_flag, false);

                            operations.push(ModificationOperation::CreateEntry {
                                options,
                                spec: EntrySpec::File {
                                    name: last_name,
                                    path,
                                    symlink_flag,
                                },
                            });
                        }
                        CompressionArg::RecursiveDirPath(path) => {
                            let last_name = last_name.take();
                            let symlink_flag = mem::replace(&mut symlink_flag, false);

                            if symlink_flag {
                                return Err(CommandError::InvalidArg(
                                    "symlink flag provided before recursive dir entry".to_string(),
                                ));
                            }

                            operations.push(ModificationOperation::CreateEntry {
                                options,
                                spec: EntrySpec::RecDir {
                                    name: last_name,
                                    path,
                                },
                            });
                        }
                    }
                }
                if symlink_flag {
                    return Err(CommandError::InvalidArg(
                        "symlink flag remaining after all entry flags processed".to_string(),
                    ));
                }
                if let Some(last_name) = last_name {
                    return Err(CommandError::InvalidArg(format!(
                        "name {last_name} remaining after all entry flags processed"
                    )));
                }

                for p in positional_paths.into_iter() {
                    operations.push(ModificationOperation::CreateEntry {
                        options,
                        spec: Self::interpret_entry_path(p)?,
                    });
                }
                Ok(ModificationSequence { operations })
            }
        }

        impl ArgvResource for ModSeqResource {
            type ArgvParseError = WrapperError<ModificationSequenceError, CommandError>;
            fn parse_argv(
                &self,
                argv: &mut VecDeque<OsString>,
            ) -> Result<ModificationSequence, Self::ArgvParseError> {
                let compression_args =
                    CompressionArgs::parse_argv(argv).map_err(WrapperError::In)?;
                compression_args.build_mod_seq().map_err(WrapperError::Out)
            }
        }

        impl PositionalArgvResource for ModSeqResource {}
    }
    use compression_args::{CompressionArgs, ModificationSequenceError};

    #[cfg(test)]
    mod test {
        use super::*;

        #[test]
        fn parse_output_type() {
            let output = OutputFlagsResource::declare(());

            assert_eq!(
                OutputType::default(),
                output.parse_argv_from_empty().unwrap()
            );

            assert_eq!(
                OutputType::Stdout { allow_tty: true },
                output.parse_argv_from(["--stdout"]).unwrap()
            );

            assert_eq!(
                OutputType::File {
                    path: "asdf".into(),
                    append: false
                },
                output.parse_argv_from(["-o", "asdf"]).unwrap()
            );
            assert_eq!(
                OutputType::File {
                    path: "asdf".into(),
                    append: true
                },
                output.parse_argv_from(["--append", "-o", "asdf"]).unwrap()
            );
        }

        #[test]
        fn parse_global_flags() {
            let global_flags = GlobalFlagsResource::declare(());

            assert_eq!(
                GlobalFlags::default(),
                global_flags.parse_argv_from_empty().unwrap(),
            );

            assert_eq!(
                GlobalFlags {
                    archive_comment: Some("asdf".into())
                },
                global_flags
                    .parse_argv_from(["--archive-comment", "asdf"])
                    .unwrap()
            );
        }

        #[test]
        fn parse_mod_seq() {
            let mod_seq = ModSeqResource::declare(());

            assert_eq!(
                ModificationSequence::default(),
                mod_seq.parse_argv_from_empty().unwrap(),
            );

            assert_eq!(
                ModificationSequence {
                    operations: vec![ModificationOperation::CreateEntry {
                        options: SimpleFileOptions::default(),
                        spec: EntrySpec::File {
                            name: None,
                            path: "file.txt".into(),
                            symlink_flag: false
                        },
                    }],
                },
                mod_seq.parse_argv_from(["-f", "file.txt"]).unwrap(),
            );
        }
    }
}

#[cfg(feature = "json")]
pub mod json_resource {
    use super::{
        GlobalFlags, GlobalFlagsResource, ModSeqResource, ModificationSequence,
        OutputFlagsResource, OutputType, Resource,
    };
    use crate::{
        args::resource::SchemaResource,
        schema::backends::{json_backend::JsonBackend, Backend},
    };

    use std::{error, ffi::OsString, fmt, path::PathBuf};

    use json::{object::Object as JsonObject, JsonValue};

    #[derive(Debug)]
    pub enum JsonSchemaError {
        InvalidType {
            val: JsonValue,
            valid_types: &'static [&'static str],
            context: &'static str,
        },
        InvalidObjectKeys {
            obj: JsonObject,
            expected_keys: &'static [&'static str],
            context: &'static str,
        },
    }

    impl fmt::Display for JsonSchemaError {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            match self {
                Self::InvalidType {
                    valid_types,
                    context,
                    val,
                } => {
                    assert!(!valid_types.is_empty());
                    let types_str: String = valid_types.join(", ");
                    write!(
                        f,
                        "{context} expected types [{types_str}], but received: {val}"
                    )
                }
                Self::InvalidObjectKeys {
                    obj,
                    expected_keys,
                    context,
                } => {
                    assert!(!expected_keys.is_empty());
                    let keys_str: String = expected_keys.join(", ");
                    let obj = JsonValue::Object(obj.clone());
                    write!(
                        f,
                        "{context} expected object keys [{keys_str}], but object was {obj}"
                    )
                }
            }
        }
    }

    impl error::Error for JsonSchemaError {}

    impl SchemaResource for OutputFlagsResource {
        type B = JsonBackend;
        type SchemaParseError = JsonSchemaError;

        fn parse_schema<'a>(
            &self,
            v: <Self::B as Backend>::Val<'a>,
        ) -> Result<OutputType, Self::SchemaParseError> {
            match v {
                JsonValue::Null => Ok(OutputType::default()),
                /* <string> => {"file": {"path": <string>, "append": false}}} */
                JsonValue::Short(path) => Ok(OutputType::File {
                    path: path.as_str().into(),
                    append: false,
                }),
                JsonValue::String(path) => Ok(OutputType::File {
                    path: path.into(),
                    append: false,
                }),
                /* <bool> => {"stdout": {"allow_tty": <bool>}} */
                JsonValue::Boolean(allow_tty) => Ok(OutputType::Stdout { allow_tty }),
                /* An object--destructure by enum case. */
                JsonValue::Object(o) => {
                    if let Some(o) = o.get("stdout") {
                        match o {
                            JsonValue::Null => Ok(OutputType::Stdout { allow_tty: false }),
                            /* {"stdout": <bool>} => {"stdout": {"allow_tty": <bool>}} */
                            JsonValue::Boolean(allow_tty) => Ok(OutputType::Stdout {
                                allow_tty: *allow_tty,
                            }),
                            /* {"stdout": {"allow_tty": <bool>}} => {"stdout": {"allow_tty": <bool>}} */
                            JsonValue::Object(o) => {
                                let allow_tty: bool = if let Some(allow_tty) = o.get("allow_tty") {
                                    match allow_tty {
                                        JsonValue::Boolean(allow_tty) => Ok(*allow_tty),
                                        JsonValue::Null => Ok(false),
                                        _ => Err(JsonSchemaError::InvalidType {
                                            val: allow_tty.clone(),
                                            valid_types: &["boolean", "null"],
                                            context: "the 'allow_tty' field in the 'stdout' case of output flags",
                                        }),
                                    }
                                } else {
                                    Ok(false)
                                }?;
                                Ok(OutputType::Stdout { allow_tty })
                            }
                            _ => Err(JsonSchemaError::InvalidType {
                                val: o.clone(),
                                valid_types: &["boolean", "object", "null"],
                                context: "the 'stdout' enum case of output flags",
                            }),
                        }
                    } else if let Some(o) = o.get("file") {
                        match o {
                            /* {"file": <string>} => {"file": {"path": <string>, append: false}} */
                            JsonValue::Short(path) => Ok(OutputType::File {
                                path: path.as_str().into(),
                                append: false,
                            }),
                            JsonValue::String(path) => Ok(OutputType::File {
                                path: path.into(),
                                append: false,
                            }),
                            /* {"file": {"path": <string>, "append": <bool>}} => {"file": {"path": <string>, append: <bool>}} */
                            JsonValue::Object(o) => {
                                let path: PathBuf = if let Some(path) = o.get("path") {
                                    match path {
                                        JsonValue::Short(path) => Ok(path.as_str().into()),
                                        JsonValue::String(path) => Ok(path.into()),
                                        _ => Err(JsonSchemaError::InvalidType {
                                            val: path.clone(),
                                            valid_types: &["string"],
                                            context: "the 'path' field in the 'file' case of output flags",
                                        }),
                                    }
                                } else {
                                    /* This *must* be provided, whereas "append" has a default. */
                                    Err(JsonSchemaError::InvalidObjectKeys {
                                        obj: o.clone(),
                                        expected_keys: &["path"],
                                        context: "the 'file' enum case of output flags",
                                    })
                                }?;
                                let append: bool = if let Some(append) = o.get("append") {
                                    match append {
                                        JsonValue::Boolean(append) => Ok(*append),
                                        JsonValue::Null => Ok(false),
                                        _ => Err(JsonSchemaError::InvalidType {
                                            val: append.clone(),
                                            valid_types: &["boolean", "null"],
                                            context:
                                                "the 'append' field in 'file' case of output flags",
                                        }),
                                    }
                                } else {
                                    Ok(false)
                                }?;
                                Ok(OutputType::File { path, append })
                            }
                            _ => Err(JsonSchemaError::InvalidType {
                                val: o.clone(),
                                valid_types: &["string", "object"],
                                context: "the 'file' enum case of output flags",
                            }),
                        }
                    } else {
                        Err(JsonSchemaError::InvalidObjectKeys {
                            obj: o,
                            expected_keys: &["stdout", "file"],
                            context:
                                "destructuring into 'file' and 'stdout' enum cases of output flags",
                        })
                    }
                }
                _ => Err(JsonSchemaError::InvalidType {
                    val: v,
                    valid_types: &["string", "boolean", "object", "null"],
                    context: "top-level value for output flags",
                }),
            }
        }
    }

    impl SchemaResource for GlobalFlagsResource {
        type B = JsonBackend;
        type SchemaParseError = JsonSchemaError;

        fn parse_schema<'a>(
            &self,
            v: <Self::B as Backend>::Val<'a>,
        ) -> Result<GlobalFlags, Self::SchemaParseError> {
            match v {
                JsonValue::Object(o) => {
                    let archive_comment: Option<OsString> = if let Some(archive_comment) =
                        o.get("archive-comment")
                    {
                        match archive_comment {
                            JsonValue::Short(archive_comment) => {
                                Ok(Some(archive_comment.as_str().into()))
                            }
                            JsonValue::String(archive_comment) => Ok(Some(archive_comment.into())),
                            JsonValue::Null => Ok(None),
                            _ => Err(JsonSchemaError::InvalidType {
                                val: archive_comment.clone(),
                                valid_types: &["string", "null"],
                                context: "the 'archive-comment' field in global flags",
                            }),
                        }
                    } else {
                        Ok(None)
                    }?;
                    Ok(GlobalFlags { archive_comment })
                }
                JsonValue::Null => Ok(GlobalFlags::default()),
                _ => Err(JsonSchemaError::InvalidType {
                    val: v.clone(),
                    valid_types: &["object", "null"],
                    context: "the top-level global flags object",
                }),
            }
        }
    }

    #[cfg(test)]
    mod test {
        use super::*;

        #[test]
        fn parse_output_type() {
            assert_eq!(
                OutputType::Stdout { allow_tty: false },
                OutputType::default()
            );

            let output = OutputFlagsResource::declare(());

            assert_eq!(
                OutputType::Stdout { allow_tty: true },
                output.parse_schema_str("true").unwrap(),
            );
            assert_eq!(
                OutputType::Stdout { allow_tty: false },
                output.parse_schema_str("false").unwrap(),
            );
            assert_eq!(
                OutputType::default(),
                output.parse_schema_str("null").unwrap(),
            );

            assert_eq!(
                OutputType::File {
                    path: "asdf".into(),
                    append: false
                },
                output.parse_schema_str("\"asdf\"").unwrap(),
            );

            assert_eq!(
                OutputType::File {
                    path: "asdf".into(),
                    append: false
                },
                output.parse_schema_str("{\"file\": \"asdf\"}").unwrap(),
            );
            assert_eq!(
                OutputType::File {
                    path: "asdf".into(),
                    append: true
                },
                output
                    .parse_schema_str("{\"file\": {\"path\": \"asdf\", \"append\": true}}")
                    .unwrap(),
            );
            assert_eq!(
                OutputType::File {
                    path: "asdf".into(),
                    append: false
                },
                output
                    .parse_schema_str("{\"file\": {\"path\": \"asdf\", \"append\": false}}")
                    .unwrap(),
            );
        }

        #[test]
        fn parse_global_flags() {
            assert_eq!(
                GlobalFlags {
                    archive_comment: None
                },
                GlobalFlags::default(),
            );

            let global_flags = GlobalFlagsResource::declare(());

            assert_eq!(
                GlobalFlags::default(),
                global_flags.parse_schema_str("null").unwrap(),
            );

            assert_eq!(
                GlobalFlags {
                    archive_comment: Some("aaaaasdf".into()),
                },
                global_flags
                    .parse_schema_str("{\"archive-comment\": \"aaaaasdf\"}")
                    .unwrap(),
            );
            assert_eq!(
                GlobalFlags {
                    archive_comment: None,
                },
                global_flags
                    .parse_schema_str("{\"archive-comment\": null}")
                    .unwrap(),
            );
        }
    }
}
