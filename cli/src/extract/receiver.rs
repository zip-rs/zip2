use std::{
    cell::RefCell,
    env, fs,
    io::{self, Read, Write},
    mem,
    path::PathBuf,
    rc::Rc,
};

use zip::read::ZipFile;

use crate::{args::extract::*, CommandError, WrapCommandErr};

pub trait EntryReceiver {
    fn receive_entry<'a>(
        &mut self,
        entry: &mut ZipFile<'a>,
        name: &str,
    ) -> Result<(), CommandError>;
    fn finalize_entries(&mut self) -> Result<(), CommandError>;
}

pub fn make_entry_receiver<'a>(
    err: Rc<RefCell<impl Write + 'a>>,
    collation: OutputCollation,
) -> Result<Box<dyn EntryReceiver + 'a>, CommandError> {
    let ret: Box<dyn EntryReceiver + 'a> = match collation {
        OutputCollation::ConcatenateStdout => Box::new(StdoutReceiver::new(err)),
        OutputCollation::Filesystem { output_dir, mkdir } => {
            let output_dir = match output_dir {
                Some(dir) => {
                    if mkdir {
                        fs::create_dir_all(&dir).wrap_err_with(|| {
                            format!("failed to create output directory {dir:?}")
                        })?;
                    }
                    dir
                }
                None => env::current_dir().wrap_err("failed to get current dir")?,
            };
            Box::new(FilesystemReceiver::new(err, output_dir))
        }
    };
    Ok(ret)
}

struct StdoutReceiver<W> {
    err: Rc<RefCell<W>>,
    stdout: io::Stdout,
}

impl<W> StdoutReceiver<W> {
    pub fn new(err: Rc<RefCell<W>>) -> Self {
        Self {
            err,
            stdout: io::stdout(),
        }
    }
}

impl<W> EntryReceiver for StdoutReceiver<W>
where
    W: Write,
{
    fn receive_entry<'a>(
        &mut self,
        entry: &mut ZipFile<'a>,
        name: &str,
    ) -> Result<(), CommandError> {
        let mut err = self.err.borrow_mut();
        writeln!(err, "receiving entry {} with name {name}", entry.name()).unwrap();
        if entry.is_dir() {
            writeln!(err, "entry is directory, ignoring").unwrap();
        } else if entry.is_symlink() {
            writeln!(err, "entry is symlink, ignoring").unwrap();
        } else {
            io::copy(entry, &mut self.stdout)
                .wrap_err_with(|| format!("failed to write entry {name} to stdout"))?;
        }
        Ok(())
    }

    fn finalize_entries(&mut self) -> Result<(), CommandError> {
        Ok(())
    }
}

struct FilesystemReceiver<W> {
    err: Rc<RefCell<W>>,
    output_dir: PathBuf,
    #[cfg(unix)]
    perms_to_set: Vec<(PathBuf, u32)>,
}

impl<W> FilesystemReceiver<W> {
    pub fn new(err: Rc<RefCell<W>>, output_dir: PathBuf) -> Self {
        Self {
            err,
            output_dir,
            #[cfg(unix)]
            perms_to_set: Vec::new(),
        }
    }
}

impl<W> EntryReceiver for FilesystemReceiver<W>
where
    W: Write,
{
    fn receive_entry<'a>(
        &mut self,
        entry: &mut ZipFile<'a>,
        name: &str,
    ) -> Result<(), CommandError> {
        let mut err = self.err.borrow_mut();
        let full_output_path = self.output_dir.join(name);
        writeln!(
            err,
            "receiving entry {} with name {name} and writing to path {full_output_path:?}",
            entry.name()
        )
        .unwrap();

        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            writeln!(
                err,
                "storing unix mode {mode} for path {full_output_path:?}"
            )
            .unwrap();
            self.perms_to_set.push((full_output_path.clone(), mode));
        }

        if entry.is_dir() {
            writeln!(err, "entry is directory, creating").unwrap();
            fs::create_dir_all(&full_output_path).wrap_err_with(|| {
                format!("failed to create directory entry at {full_output_path:?}")
            })?;
        } else if entry.is_symlink() {
            let mut target: Vec<u8> = Vec::with_capacity(entry.size().try_into().unwrap());
            entry.read_to_end(&mut target).wrap_err_with(|| {
                format!(
                    "failed to read symlink target from zip archive entry {}",
                    entry.name()
                )
            })?;

            #[cfg(unix)]
            {
                use std::{
                    ffi::OsString,
                    os::unix::{ffi::OsStringExt, fs::symlink},
                };
                let target = OsString::from_vec(target);
                writeln!(err, "entry is symlink to {target:?}, creating").unwrap();
                symlink(&target, &full_output_path).wrap_err_with(|| {
                    format!(
                        "failed to create symlink at {full_output_path:?} with target {target:?}"
                    )
                })?;
            }
            #[cfg(not(unix))]
            {
                /* FIXME: non-unix symlink extraction not yet supported! */
                todo!("TODO: cannot create symlink for entry {name} on non-unix yet!");
            }
        } else {
            writeln!(err, "entry is file, creating").unwrap();
            if let Some(containing_dir) = full_output_path.parent() {
                fs::create_dir_all(containing_dir).wrap_err_with(|| {
                    format!("failed to create parent dirs for file at {full_output_path:?}")
                })?;
            } else {
                writeln!(err, "entry had no parent dir (in root dir?)").unwrap();
            }
            let mut outfile = fs::File::create(&full_output_path)
                .wrap_err_with(|| format!("failed to create file at {full_output_path:?}"))?;
            io::copy(entry, &mut outfile).wrap_err_with(|| {
                format!(
                    "failed to copy file contents from {} to {full_output_path:?}",
                    entry.name()
                )
            })?;
        }
        Ok(())
    }

    fn finalize_entries(&mut self) -> Result<(), CommandError> {
        #[cfg(unix)]
        {
            use std::{cmp::Reverse, os::unix::fs::PermissionsExt};

            let mut perms_to_set = mem::take(&mut self.perms_to_set);
            perms_to_set.sort_unstable_by_key(|(path, _)| Reverse(path.clone()));
            for (path, mode) in perms_to_set.into_iter() {
                let perms = fs::Permissions::from_mode(mode);
                fs::set_permissions(&path, perms.clone())
                    .wrap_err_with(|| format!("error setting perms {perms:?} for path {path:?}"))?;
            }
        }
        Ok(())
    }
}
