use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{HashMap, HashSet},
    env, fs,
    io::{self, Read, Seek, Write},
    mem,
    path::PathBuf,
    rc::Rc,
};

use zip::{read::ZipFile, CompressionMethod};

use super::matcher::{CompiledMatcher, EntryMatcher};
use super::transform::{CompiledTransformer, NameTransformer};
use crate::{args::extract::*, CommandError, WrapCommandErr};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntryData<'a> {
    pub name: &'a str,
    pub kind: EntryKind,
    pub compression: CompressionMethod,
    pub unix_mode: Option<u32>,
    pub size: u64,
}

impl<'a> EntryData<'a> {
    #[inline(always)]
    pub fn from_entry<'b>(entry: &'a ZipFile<'b>) -> Self {
        Self {
            name: entry.name(),
            kind: if entry.is_dir() {
                EntryKind::Dir
            } else if entry.is_symlink() {
                EntryKind::Symlink
            } else {
                EntryKind::File
            },
            compression: entry.compression(),
            unix_mode: entry.unix_mode(),
            size: entry.size(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OutputName(pub String);

impl OutputName {
    pub fn default_name() -> Self {
        Self("default".to_string())
    }
}

pub struct ParsedEntrySpecArg {
    matcher: Option<CompiledMatcher>,
    transforms: Option<CompiledTransformer>,
    output_name: OutputName,
}

impl ParsedEntrySpecArg {
    pub fn from_entry_spec(spec: EntrySpec) -> Result<Self, CommandError> {
        let EntrySpec {
            match_expr,
            name_transforms,
            content_transform,
        } = spec;
        let matcher = match match_expr {
            None => None,
            Some(expr) => Some(CompiledMatcher::from_arg(expr)?),
        };
        let transforms = if name_transforms.is_empty() {
            None
        } else {
            Some(CompiledTransformer::from_arg()?)
        };
        let output_name = match content_transform {
            ContentTransform::Extract { name } => name
                .map(OutputName)
                .unwrap_or_else(OutputName::default_name),
        };
        Ok(Self {
            matcher,
            transforms,
            output_name,
        })
    }
}

pub struct ConcatEntry {
    pub matcher: Option<CompiledMatcher>,
    pub stream: Rc<RefCell<dyn Write>>,
}

pub struct ExtractEntry {
    pub matcher: Option<CompiledMatcher>,
    pub transforms: Option<CompiledTransformer>,
    pub recv: Rc<dyn EntryReceiver>,
}

pub enum CompiledEntrySpec {
    Concat(ConcatEntry),
    Extract(ExtractEntry),
}

pub struct ParsedNamedOutputs {
    concats: HashMap<OutputName, Rc<RefCell<dyn Write>>>,
    extracts: HashMap<OutputName, Rc<dyn EntryReceiver>>,
}

pub fn process_entry_and_output_specs(
    entry_specs: impl IntoIterator<Item = EntrySpec>,
    output_specs: OutputSpecs,
) -> Result<Vec<CompiledEntrySpec>, CommandError> {
    let entry_specs: Vec<ParsedEntrySpecArg> = entry_specs
        .into_iter()
        .map(ParsedEntrySpecArg::from_entry_spec)
        .collect::<Result<_, _>>()?;
    assert!(!entry_specs.is_empty());
    let parsed_outputs = ParsedNamedOutputs::from_output_specs(output_specs)?;
    parsed_outputs.process_entry_specs_for_outputs(entry_specs)
}

impl ParsedNamedOutputs {
    pub fn process_entry_specs_for_outputs(
        self,
        args: impl IntoIterator<Item = ParsedEntrySpecArg>,
    ) -> Result<Vec<CompiledEntrySpec, CommandError>> {
        args.into_iter()
            .map(|arg| self.lookup_entry_spec_arg(arg))
            .collect()
    }

    fn lookup_entry_spec_arg(
        &self,
        arg: ParsedEntrySpecArg,
    ) -> Result<CompiledEntrySpec, CommandError> {
        let ParsedEntrySpecArg {
            matcher,
            transforms,
            output_name,
        } = arg;
        if let Some(stream) = self.concats.get(&output_name) {
            if transforms.is_some() {
                return Err(CommandError::InvalidArg(
                    format!("entry name transforms {transforms:?} do not apply to concat output {output_name:?}")
                ));
            }
            return Ok(CompiledEntrySpec::Concat(ConcatEntry {
                matcher,
                stream: stream.clone(),
            }));
        }
        let Some(recv) = self.extracts.get(&output_name) else {
            return Err(CommandError::InvalidArg(format!(
                "output name {output_name:?} was not found"
            )));
        };
        Ok(CompiledEntrySpec::Extract(ExtractEntry {
            matcher,
            transforms,
            recv: recv.clone(),
        }))
    }

    fn add_stdout(
        seen_stdout: &mut bool,
        name: OutputName,
        seen_names: &mut HashSet<OutputName>,
        concats: &mut HashMap<OutputName, Rc<RefCell<dyn Write>>>,
    ) -> Result<(), CommandError> {
        if *seen_stdout {
            return Err(CommandError::InvalidArg(
                "--stdout output provided for more than one receiver".to_string(),
            ));
        }
        if seen_names.contains(&name) {
            return Err(CommandError::InvalidArg(format!(
                "output name {name:?} provided more than once"
            )));
        }
        assert!(!concats.contains(&name));

        let handle: Rc<RefCell<dyn Write>> = Rc::new(RefCell::new(io::stdout()));

        *seen_stdout = true;
        assert!(seen_names.insert(name.clone()));
        assert!(concats.insert(name, handle).is_none());
        Ok(())
    }

    fn add_file(
        path: PathBuf,
        append: bool,
        name: OutputName,
        seen_files: &mut HashSet<PathBuf>,
        seen_names: &mut HashSet<OutputName>,
        concats: &mut HashMap<OutputName, Rc<RefCell<dyn Write>>>,
    ) -> Result<(), CommandError> {
        if seen_names.contains(&name) {
            return Err(CommandError::InvalidArg(format!(
                "output name {name:?} provided more than once"
            )));
        }
        assert!(!concats.contains(&name));
        let canon_path = path
            .canonicalize()
            .wrap_err_with(|| format!("canonicalizing path {path:?} failed"))?;
        if seen_files.contains(&canon_path) {
            return Err(CommandError::InvalidArg(format!(
                "canonical output file path {canon_path:?} provided more than once"
            )));
        }

        let handle: Rc<RefCell<dyn Write>> = {
            let mut f: fs::File = if append {
                fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .open(&path)
                    .wrap_err_with(|| format!("failed to open file for append at {path:?}"))?
            } else {
                fs::File::create(&path)
                    .wrap_err_with(|| format!("failed to open file with truncation at {path:?}"))?
            };
            f.seek(io::SeekFrom::End(0))
                .wrap_err_with(|| format!("failed to seek to end of opened file {f:?}"))?;
            Rc::new(RefCell::new(f))
        };

        assert!(seen_files.insert(canon_path));
        assert!(seen_names.insert(name.clone()));
        assert!(concats.insert(name, handle).is_none());
        Ok(())
    }

    fn add_dir(
        output_dir: PathBuf,
        mkdir: bool,
        name: OutputName,
        seen_dirs: &mut HashSet<PathBuf>,
        seen_names: &mut HashSet<OutputName>,
        extracts: &mut HashMap<OutputName, Rc<dyn EntryReceiver>>,
    ) -> Result<(), CommandError> {
        if seen_names.contains(&name) {
            return Err(CommandError::InvalidArg(format!(
                "output name {name:?} provided more than once"
            )));
        }
        assert!(!extracts.contains(&name));
        let canon_path = path
            .canonicalize()
            .wrap_err_with(|| format!("canonicalizing dir path {path:?} failed"))?;
        if seen_dirs.contains(&canon_path) {
            return Err(CommandError::InvalidArg(format!(
                "canonical output dir path {canon_path:?} provided more than once"
            )));
        }

        let handle: Rc<dyn EntryReceiver> = {
            if mkdir {
                fs::create_dir_all(&output_dir).wrap_err_with(|| {
                    format!("failed to create output directory {output_dir:?}")
                })?;
            };
            let d = FilesystemReceiver::new(output_dir);
            Rc::new(d)
        };

        assert!(seen_dirs.insert(canon_path));
        assert!(seen_names.insert(name.clone()));
        assert!(extracts.insert(name, handle).is_none());
        Ok(())
    }

    pub fn from_output_specs(spec: OutputSpecs) -> Result<Self, CommandError> {
        let OutputSpecs { default, named } = spec;

        let mut concats: HashMap<OutputName, Rc<RefCell<dyn Write>>> = HashMap::new();
        let mut extracts: HashMap<OutputName, Rc<dyn EntryReceiver>> = HashMap::new();

        let mut seen_stdout: bool = false;
        let mut seen_files: HashSet<PathBuf> = HashSet::new();
        let mut seen_dirs: HashSet<PathBuf> = HashSet::new();
        let mut seen_names: HashSet<OutputName> = HashSet::new();

        if let Some(default) = default {
            match default {
                OutputCollation::ConcatenateStdout => {
                    Self::add_stdout(
                        &mut seen_stdout,
                        OutputName::default_name(),
                        &mut seen_names,
                        &mut concats,
                    )?;
                }
                OutputCollation::ConcatenateFile { path, append } => {
                    Self::add_file(
                        path,
                        append,
                        OutputName::default_name(),
                        &mut seen_files,
                        &mut seen_names,
                        &mut concats,
                    )?;
                }
                OutputCollation::Filesystem { output_dir, mkdir } => {
                    Self::add_dir(
                        output_dir,
                        mkdir,
                        OutputName::default_name(),
                        &mut seen_dirs,
                        &mut seen_names,
                        &mut extracts,
                    )?;
                }
            }
        }
        for NamedOutput { name, output } in named.into_iter() {
            match output {
                OutputCollation::ConcatenateStdout => {
                    Self::add_stdout(&mut seen_stdout, name, &mut seen_names, &mut concats)?;
                }
                OutputCollation::ConcatenateFile { path, append } => {
                    Self::add_file(
                        path,
                        append,
                        name,
                        &mut seen_files,
                        &mut seen_names,
                        &mut concats,
                    )?;
                }
                OutputCollation::Filesystem { output_dir, mkdir } => {
                    Self::add_dir(
                        output_dir,
                        mkdir,
                        name,
                        &mut seen_dirs,
                        &mut seen_names,
                        &mut extracts,
                    )?;
                }
            }
        }

        Ok(Self { concats, extracts })
    }
}

pub trait EntryReceiver {
    fn generate_entry_handle<'s>(&self, name: Cow<'s, str>)
        -> Result<Box<dyn Write>, CommandError>;

    fn finalize_entries(&self) -> Result<(), CommandError>;
}

struct FilesystemReceiver {
    output_dir: PathBuf,
    #[cfg(unix)]
    perms_to_set: RefCell<Vec<(PathBuf, u32)>>,
}

impl FilesystemReceiver {
    pub fn new(output_dir: PathBuf) -> Self {
        Self {
            output_dir,
            #[cfg(unix)]
            perms_to_set: RefCell::new(Vec::new()),
        }
    }
}

impl EntryReceiver for FilesystemReceiver {
    fn generate_entry_handle<'s>(
        &self,
        name: Cow<'s, str>,
    ) -> Result<Box<dyn Write>, CommandError> {
        todo!("wow!")
    }

    /* fn receive_entry<'a>( */
    /*     &mut self, */
    /*     entry: &mut ZipFile<'a>, */
    /*     name: &str, */
    /* ) -> Result<(), CommandError> { */
    /*     let mut err = self.err.borrow_mut(); */
    /*     let full_output_path = self.output_dir.join(name); */
    /*     writeln!( */
    /*         err, */
    /*         "receiving entry {} with name {name} and writing to path {full_output_path:?}", */
    /*         entry.name() */
    /*     ) */
    /*     .unwrap(); */

    /*     #[cfg(unix)] */
    /*     if let Some(mode) = entry.unix_mode() { */
    /*         writeln!( */
    /*             err, */
    /*             "storing unix mode {mode} for path {full_output_path:?}" */
    /*         ) */
    /*         .unwrap(); */
    /*         self.perms_to_set */
    /*             .borrow_mut() */
    /*             .push((full_output_path.clone(), mode)); */
    /*     } */

    /*     if entry.is_dir() { */
    /*         writeln!(err, "entry is directory, creating").unwrap(); */
    /*         fs::create_dir_all(&full_output_path).wrap_err_with(|| { */
    /*             format!("failed to create directory entry at {full_output_path:?}") */
    /*         })?; */
    /*     } else if entry.is_symlink() { */
    /*         let mut target: Vec<u8> = Vec::with_capacity(entry.size().try_into().unwrap()); */
    /*         entry.read_to_end(&mut target).wrap_err_with(|| { */
    /*             format!( */
    /*                 "failed to read symlink target from zip archive entry {}", */
    /*                 entry.name() */
    /*             ) */
    /*         })?; */

    /*         #[cfg(unix)] */
    /*         { */
    /*             use std::{ */
    /*                 ffi::OsString, */
    /*                 os::unix::{ffi::OsStringExt, fs::symlink}, */
    /*             }; */
    /*             let target = OsString::from_vec(target); */
    /*             writeln!(err, "entry is symlink to {target:?}, creating").unwrap(); */
    /*             symlink(&target, &full_output_path).wrap_err_with(|| { */
    /*                 format!( */
    /*                     "failed to create symlink at {full_output_path:?} with target {target:?}" */
    /*                 ) */
    /*             })?; */
    /*         } */
    /*         #[cfg(not(unix))] */
    /*         { */
    /*             /\* FIXME: non-unix symlink extraction not yet supported! *\/ */
    /*             todo!("TODO: cannot create symlink for entry {name} on non-unix yet!"); */
    /*         } */
    /*     } else { */
    /*         writeln!(err, "entry is file, creating").unwrap(); */
    /*         if let Some(containing_dir) = full_output_path.parent() { */
    /*             fs::create_dir_all(containing_dir).wrap_err_with(|| { */
    /*                 format!("failed to create parent dirs for file at {full_output_path:?}") */
    /*             })?; */
    /*         } else { */
    /*             writeln!(err, "entry had no parent dir (in root dir?)").unwrap(); */
    /*         } */
    /*         let mut outfile = fs::File::create(&full_output_path) */
    /*             .wrap_err_with(|| format!("failed to create file at {full_output_path:?}"))?; */
    /*         io::copy(entry, &mut outfile).wrap_err_with(|| { */
    /*             format!( */
    /*                 "failed to copy file contents from {} to {full_output_path:?}", */
    /*                 entry.name() */
    /*             ) */
    /*         })?; */
    /*     } */
    /*     Ok(()) */
    /* } */

    fn finalize_entries(&mut self) -> Result<(), CommandError> {
        #[cfg(unix)]
        {
            use std::{cmp::Reverse, os::unix::fs::PermissionsExt};

            let mut perms_to_set = mem::take(&mut *self.perms_to_set.borrow_mut());
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
