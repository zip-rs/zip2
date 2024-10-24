use std::{
    fs,
    io::{self, Cursor, IsTerminal, Seek, Write},
    mem,
    path::Path,
};

use zip::{
    unstable::path_to_string,
    write::{SimpleFileOptions, ZipWriter},
    CompressionMethod, ZIP64_BYTES_THR,
};

use crate::{args::compress::*, CommandError, OutputHandle, WrapCommandErr};

fn enter_recursive_dir_entries(
    err: &mut impl Write,
    base_rename: Option<String>,
    root: &Path,
    writer: &mut ZipWriter<impl Write + Seek>,
    options: SimpleFileOptions,
) -> Result<(), CommandError> {
    let base_dirname: String = base_rename
        .unwrap_or_else(|| path_to_string(root).into())
        .trim_end_matches('/')
        .to_string();
    writeln!(
        err,
        "writing top-level directory entry for {base_dirname:?}"
    )
    .unwrap();
    writer
        .add_directory(&base_dirname, options)
        .wrap_err_with(|| format!("error adding top-level directory entry {base_dirname}"))?;

    let mut readdir_stack: Vec<(fs::ReadDir, String)> = vec![(
        fs::read_dir(root)
            .wrap_err_with(|| format!("error reading directory contents for {}", root.display()))?,
        base_dirname,
    )];
    while let Some((mut readdir, top_component)) = readdir_stack.pop() {
        if let Some(dir_entry) = readdir
            .next()
            .transpose()
            .wrap_err("reading next dir entry")?
        {
            let mut components: Vec<&str> = readdir_stack.iter().map(|(_, s)| s.as_ref()).collect();
            components.push(&top_component);

            let entry_basename: String = dir_entry.file_name().into_string().map_err(|name| {
                CommandError::InvalidArg(format!("failed to decode basename {name:?}"))
            })?;
            components.push(&entry_basename);
            let full_path: String = components.join("/");
            readdir_stack.push((readdir, top_component));

            let file_type = dir_entry.file_type().wrap_err_with(|| {
                format!("failed to read file type for dir entry {dir_entry:?}")
            })?;
            if file_type.is_symlink() {
                let target: String = path_to_string(
                    fs::read_link(dir_entry.path())
                        .wrap_err_with(|| format!("failed to read symlink from {dir_entry:?}"))?,
                )
                .into();
                if target.len() > ZIP64_BYTES_THR.try_into().unwrap() {
                    return Err(CommandError::InvalidArg(format!(
                        "symlink target for {full_path} is over {ZIP64_BYTES_THR} bytes (was: {})",
                        target.len()
                    )));
                }
                writeln!(
                    err,
                    "writing recursive symlink entry with name {full_path:?} and target {target:?}"
                )
                .unwrap();
                writer
                    .add_symlink(&full_path, &target, options)
                    .wrap_err_with(|| format!("error adding symlink from {full_path}->{target}"))?;
            } else if file_type.is_file() {
                writeln!(err, "writing recursive file entry with name {full_path:?}").unwrap();
                let mut f = fs::File::open(dir_entry.path()).wrap_err_with(|| {
                    format!("error opening file for {full_path} from dir entry {dir_entry:?}")
                })?;
                /* Get the length of the file before reading it and set large_file if needed. */
                let input_len: u64 = f
                    .metadata()
                    .wrap_err_with(|| format!("error reading file metadata for {f:?}"))?
                    .len();
                let maybe_large_file_options = if input_len > ZIP64_BYTES_THR {
                    writeln!(
                        err,
                        "temporarily ensuring .large_file(true) for current entry"
                    )
                    .unwrap();
                    options.large_file(true)
                } else {
                    options
                };
                writer
                    .start_file(&full_path, maybe_large_file_options)
                    .wrap_err_with(|| format!("error creating file entry for {full_path}"))?;
                io::copy(&mut f, writer).wrap_err_with(|| {
                    format!("error copying content for {full_path} from file {f:?}")
                })?;
            } else {
                assert!(file_type.is_dir());
                writeln!(
                    err,
                    "writing recursive directory entry with name {full_path:?}"
                )
                .unwrap();
                writer
                    .add_directory(&full_path, options)
                    .wrap_err_with(|| format!("failed to create directory entry {full_path}"))?;
                writeln!(
                    err,
                    "adding subdirectories depth-first for recursive directory entry {entry_basename:?}"
                ).unwrap();
                let new_readdir = fs::read_dir(dir_entry.path()).wrap_err_with(|| {
                    format!("failed to read recursive directory contents from {dir_entry:?}")
                })?;
                readdir_stack.push((new_readdir, entry_basename));
            }
        }
    }
    Ok(())
}

pub fn execute_compress(mut err: impl Write, args: Compress) -> Result<(), CommandError> {
    let Compress {
        output,
        archive_comment,
        args,
        positional_paths,
    } = args;

    let (out, do_append) = match output {
        OutputType::File { path, append } => {
            if append {
                writeln!(
                    err,
                    "reading compressed zip from output file path {path:?} for append"
                )
                .unwrap();
                match fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(false)
                    .open(&path)
                {
                    Ok(f) => {
                        writeln!(err, "output zip file existed, appending").unwrap();
                        (OutputHandle::File(f), true)
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        writeln!(
                            err,
                            "output zip file did not exist, creating new file instead of appending"
                        )
                        .unwrap();
                        let out =
                            OutputHandle::File(fs::File::create(&path).wrap_err_with(|| {
                                format!("failed to create new zip output file at {path:?}")
                            })?);
                        (out, false)
                    }
                    Err(e) => {
                        return Err(e).wrap_err_with(|| {
                            format!(
                                "unexpected error reading zip output file for append at {path:?}"
                            )
                        });
                    }
                }
            } else {
                writeln!(err, "writing compressed zip to output file path {path:?}").unwrap();
                let out = OutputHandle::File(fs::File::create(&path).wrap_err_with(|| {
                    format!("failed to create output file at {}", path.display())
                })?);
                (out, false)
            }
        }
        OutputType::Stdout { allow_tty } => {
            writeln!(
                err,
                "writing to stdout and buffering compressed zip in memory"
            )
            .unwrap();
            if io::stdout().is_terminal() && !allow_tty {
                /* TODO: maybe figure out some way to ensure --stdout is still the correct flag */
                return Err(CommandError::InvalidArg(
                    "stdout is a tty, but --stdout was not set".to_string(),
                ));
            }
            let out = OutputHandle::InMem(Cursor::new(Vec::new()));
            (out, false)
        }
    };
    let mut writer = if do_append {
        ZipWriter::new_append(out)
            .wrap_err("failed to initialize zip writer from existing zip file for append")?
    } else {
        ZipWriter::new(out)
    };

    if let Some(comment) = archive_comment {
        writeln!(err, "comment was provided: {comment:?}").unwrap();
        let comment = comment.into_encoded_bytes();
        writer.set_raw_comment(comment.into());
    }

    let mut options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .large_file(false);
    writeln!(err, "default zip entry options: {options:?}").unwrap();
    let mut last_name: Option<String> = None;
    let mut symlink_flag: bool = false;

    for arg in args.into_iter() {
        match arg {
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
                writeln!(err, "setting compression method {method:?}").unwrap();
                options = options.compression_method(method);
            }
            CompressionArg::Level(CompressionLevel(level)) => {
                writeln!(err, "setting compression level {level:?}").unwrap();
                options = options.compression_level(Some(level));
            }
            CompressionArg::UnixPermissions(UnixPermissions(mode)) => {
                writeln!(err, "setting file mode {mode:#o}").unwrap();
                options = options.unix_permissions(mode);
            }
            CompressionArg::LargeFile(large_file) => {
                writeln!(err, "setting large file flag to {large_file:?}").unwrap();
                options = options.large_file(large_file);
            }
            CompressionArg::Name(name) => {
                writeln!(err, "setting name of next entry to {name:?}").unwrap();
                if let Some(last_name) = last_name {
                    return Err(CommandError::InvalidArg(format!(
                        "got two names before an entry: {last_name} and {name}"
                    )));
                }
                last_name = Some(name);
            }
            CompressionArg::Dir => {
                writeln!(err, "writing dir entry").unwrap();
                if symlink_flag {
                    return Err(CommandError::InvalidArg(
                        "symlink flag provided before dir entry".to_string(),
                    ));
                }
                let dirname = last_name.take().ok_or_else(|| {
                    CommandError::InvalidArg("no name provided before dir entry".to_string())
                })?;
                writer
                    .add_directory(&dirname, options)
                    .wrap_err_with(|| format!("failed to create dir entry {dirname}"))?;
            }
            CompressionArg::Symlink => {
                writeln!(err, "setting symlink flag for next entry").unwrap();
                if symlink_flag {
                    /* TODO: make this a warning? */
                    return Err(CommandError::InvalidArg(
                        "symlink flag provided twice before entry".to_string(),
                    ));
                }
                symlink_flag = true;
            }
            CompressionArg::Immediate(data) => {
                let name = last_name.take().ok_or_else(|| {
                    CommandError::InvalidArg(format!(
                        "no name provided for immediate data {data:?}"
                    ))
                })?;
                /* It's highly unlikely any OS allows process args of this length, so even though
                 * we're using rust's env::args_os() and it would be very impressive for an attacker
                 * to get CLI args to overflow, it seems likely to be inefficient in any case, and
                 * very unlikely to be useful, so exit with a clear error. */
                if data.len() > ZIP64_BYTES_THR.try_into().unwrap() {
                    return Err(CommandError::InvalidArg(format!(
                        "length of immediate data argument is {}; use a file for inputs over {} bytes",
                        data.len(),
                        ZIP64_BYTES_THR
                    )));
                };
                if symlink_flag {
                    /* This is a symlink entry. */
                    let target = data.into_string().map_err(|target| {
                        CommandError::InvalidArg(format!(
                            "failed to decode immediate symlink target {target:?}"
                        ))
                    })?;
                    writeln!(
                        err,
                        "writing immediate symlink entry with name {name:?} and target {target:?}"
                    )
                    .unwrap();
                    /* TODO: .add_symlink() should support OsString targets! */
                    writer
                        .add_symlink(&name, &target, options)
                        .wrap_err_with(|| {
                            format!("failed to created symlink entry {name}->{target}")
                        })?;
                    symlink_flag = false;
                } else {
                    /* This is a file entry. */
                    writeln!(
                        err,
                        "writing immediate file entry with name {name:?} and data {data:?}"
                    )
                    .unwrap();
                    let data = data.into_encoded_bytes();
                    writer
                        .start_file(&name, options)
                        .wrap_err_with(|| format!("failed to create file entry {name}"))?;
                    writer.write_all(data.as_ref()).wrap_err_with(|| {
                        format!(
                            "failed writing immediate data of length {} to file entry {name}",
                            data.len()
                        )
                    })?;
                }
            }
            CompressionArg::FilePath(path) => {
                let name = last_name
                    .take()
                    .unwrap_or_else(|| path_to_string(&path).into());
                if symlink_flag {
                    /* This is a symlink entry. */
                    let target: String =
                        path_to_string(fs::read_link(&path).wrap_err_with(|| {
                            format!("failed to read symlink from path {}", path.display())
                        })?)
                        .into();
                    /* Similarly to immediate data arguments, we're simply not going to support
                     * symlinks over this length, which should be impossible anyway. */
                    if target.len() > ZIP64_BYTES_THR.try_into().unwrap() {
                        return Err(CommandError::InvalidArg(format!(
                            "symlink target for {name} is over {ZIP64_BYTES_THR} bytes (was: {})",
                            target.len()
                        )));
                    }
                    writeln!(err, "writing symlink entry from path {path:?} with name {name:?} and target {target:?}").unwrap();
                    writer
                        .add_symlink(&name, &target, options)
                        .wrap_err_with(|| {
                            format!("failed to create symlink entry for {name}->{target}")
                        })?;
                    symlink_flag = false;
                } else {
                    /* This is a file entry. */
                    writeln!(
                        err,
                        "writing file entry from path {path:?} with name {name:?}"
                    )
                    .unwrap();
                    let mut f = fs::File::open(&path).wrap_err_with(|| {
                        format!("error opening file for {name} at {}", path.display())
                    })?;
                    /* Get the length of the file before reading it and set large_file if needed. */
                    let input_len: u64 = f
                        .metadata()
                        .wrap_err_with(|| format!("error reading file metadata for {f:?}"))?
                        .len();
                    writeln!(err, "entry is {input_len} bytes long").unwrap();
                    let maybe_large_file_options = if input_len > ZIP64_BYTES_THR {
                        writeln!(
                            err,
                            "temporarily ensuring .large_file(true) for current entry"
                        )
                        .unwrap();
                        options.large_file(true)
                    } else {
                        options
                    };
                    writer
                        .start_file(&name, maybe_large_file_options)
                        .wrap_err_with(|| format!("error creating file entry for {name}"))?;
                    io::copy(&mut f, &mut writer).wrap_err_with(|| {
                        format!("error copying content for {name} from file {f:?}")
                    })?;
                }
            }
            CompressionArg::RecursiveDirPath(r) => {
                if symlink_flag {
                    return Err(CommandError::InvalidArg(
                        "symlink flag provided before recursive dir entry".to_string(),
                    ));
                }
                writeln!(
                    err,
                    "writing recursive dir entries for path {r:?} with name {last_name:?}"
                )
                .unwrap();
                enter_recursive_dir_entries(&mut err, last_name.take(), &r, &mut writer, options)?;
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

    for pos_arg in positional_paths.into_iter() {
        let file_type = fs::symlink_metadata(&pos_arg)
            .wrap_err_with(|| format!("failed to read metadata from path {}", pos_arg.display()))?
            .file_type();
        if file_type.is_symlink() {
            let target = fs::read_link(&pos_arg).wrap_err_with(|| {
                format!("failed to read symlink content from {}", pos_arg.display())
            })?;
            writeln!(
                err,
                "writing positional symlink entry with path {pos_arg:?} and target {target:?}"
            )
            .unwrap();
            writer
                .add_symlink_from_path(&pos_arg, &target, options)
                .wrap_err_with(|| {
                    format!(
                        "failed to create symlink entry for {}->{}",
                        pos_arg.display(),
                        target.display()
                    )
                })?;
        } else if file_type.is_file() {
            writeln!(err, "writing positional file entry with path {pos_arg:?}").unwrap();
            let mut f = fs::File::open(&pos_arg)
                .wrap_err_with(|| format!("failed to open file at {}", pos_arg.display()))?;
            /* Get the length of the file before reading it and set large_file if needed. */
            let input_len: u64 = f
                .metadata()
                .wrap_err_with(|| format!("error reading file metadata for {f:?}"))?
                .len();
            let maybe_large_file_options = if input_len > ZIP64_BYTES_THR {
                writeln!(
                    err,
                    "temporarily ensuring .large_file(true) for current entry"
                )
                .unwrap();
                options.large_file(true)
            } else {
                options
            };
            writer
                .start_file_from_path(&pos_arg, maybe_large_file_options)
                .wrap_err_with(|| format!("failed to create file entry {}", pos_arg.display()))?;
            io::copy(&mut f, &mut writer)
                .wrap_err_with(|| format!("failed to copy file contents from {f:?}"))?;
        } else {
            assert!(file_type.is_dir());
            writeln!(
                err,
                "writing positional recursive dir entry for {pos_arg:?}"
            )
            .unwrap();
            enter_recursive_dir_entries(&mut err, None, &pos_arg, &mut writer, options)?;
        }
    }

    let handle = writer
        .finish()
        .wrap_err("failed to write zip to output handle")?;
    match handle {
        OutputHandle::File(f) => {
            let archive_len: u64 = f
                .metadata()
                .wrap_err_with(|| format!("failed reading metadata from file {f:?}"))?
                .len();
            writeln!(err, "file archive {f:?} was {archive_len} bytes").unwrap();
            mem::drop(f); /* Superfluous explicit drop. */
        }
        OutputHandle::InMem(mut cursor) => {
            let archive_len: u64 = cursor.position();
            writeln!(err, "in-memory archive was {archive_len} bytes").unwrap();
            cursor.rewind().wrap_err("failed to rewind cursor")?;
            let mut stdout = io::stdout().lock();
            io::copy(&mut cursor, &mut stdout)
                .wrap_err("failed to copy {archive_len} byte archive to stdout")?;
        }
    }

    Ok(())
}
