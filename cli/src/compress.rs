use std::{
    ffi::OsString,
    fs,
    io::{self, Cursor, IsTerminal, Seek, Write},
    mem,
    path::{Path, PathBuf},
};

use zip::{
    unstable::path_to_string,
    write::{SimpleFileOptions, ZipWriter},
    CompressionMethod, ZIP64_BYTES_THR,
};

use crate::{args::compress::*, CommandError, OutputHandle, WrapCommandErr};

impl EntrySpec {
    pub fn create_entry(
        self,
        writer: &mut ZipWriter<impl Write + Seek>,
        options: SimpleFileOptions,
        mut err: impl Write,
    ) -> Result<(), CommandError> {
        match self {
            Self::Dir { name } => writer
                .add_directory(&name, options)
                .wrap_err_with(|| format!("failed to create dir entry {name}")),
            Self::Immediate {
                name,
                data,
                symlink_flag,
            } => {
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
                        })
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
                    })
                }
            }
            Self::File {
                name,
                path,
                symlink_flag,
            } => {
                let name = name.unwrap_or_else(|| path_to_string(&path).into());
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
                        })
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
                    io::copy(&mut f, writer)
                        .wrap_err_with(|| {
                            format!("error copying content for {name} from file {f:?}")
                        })
                        .map(|_| ())
                }
            }
            Self::RecDir { name, path } => {
                writeln!(
                    err,
                    "writing recursive dir entries for path {path:?} with name {name:?}"
                )
                .unwrap();
                enter_recursive_dir_entries(&mut err, name, &path, writer, options)
            }
        }
    }
}

impl ModificationOperation {
    pub fn invoke(
        self,
        writer: &mut ZipWriter<impl Write + Seek>,
        err: impl Write,
    ) -> Result<(), CommandError> {
        match self {
            Self::CreateEntry { options, spec } => spec.create_entry(writer, options, err),
        }
    }
}

impl ModificationSequence {
    pub fn invoke(
        self,
        writer: &mut ZipWriter<impl Write + Seek>,
        mut err: impl Write,
    ) -> Result<(), CommandError> {
        let Self { operations } = self;
        for op in operations.into_iter() {
            op.invoke(writer, &mut err)?;
        }
        Ok(())
    }
}

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
        global_flags,
        mod_seq,
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

    let GlobalFlags { archive_comment } = global_flags;
    if let Some(comment) = archive_comment {
        writeln!(err, "comment was provided: {comment:?}").unwrap();
        let comment = comment.into_encoded_bytes();
        writer.set_raw_comment(comment.into());
    }

    mod_seq.invoke(&mut writer, &mut err)?;

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
