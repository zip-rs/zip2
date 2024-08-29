use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{HashMap, HashSet},
    fmt, fs,
    io::{self, Seek, Write},
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
struct OutputName(pub String);

impl OutputName {
    pub fn default_name() -> Self {
        Self("default".to_string())
    }
}

struct ParsedEntrySpecArg {
    pub matcher: Option<CompiledMatcher>,
    pub transforms: Option<CompiledTransformer>,
    pub output_name: OutputName,
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
            Some(CompiledTransformer::from_arg(name_transforms)?)
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

pub struct ConcatEntry<'w> {
    pub matcher: Option<CompiledMatcher>,
    pub stream: Rc<RefCell<dyn Write + 'w>>,
}

impl<'w> fmt::Debug for ConcatEntry<'w> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "ConcatEntry {{ matcher: {:?}, stream: {:p} }}",
            &self.matcher, &self.stream
        )
    }
}

impl<'w> ConcatEntry<'w> {
    pub fn do_match<'a>(&self, data: &EntryData<'a>) -> Option<&Rc<RefCell<dyn Write + 'w>>> {
        if self
            .matcher
            .as_ref()
            .map(|m| m.matches(data))
            .unwrap_or(true)
        {
            Some(&self.stream)
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub struct ExtractEntry<'w> {
    pub matcher: Option<CompiledMatcher>,
    pub transforms: Option<CompiledTransformer>,
    pub recv: Rc<dyn EntryReceiver + 'w>,
}

impl<'w> ExtractEntry<'w> {
    pub fn do_match_and_transform<'a>(
        &self,
        data: &EntryData<'a>,
    ) -> Option<(Cow<'a, str>, &Rc<dyn EntryReceiver + 'w>)> {
        if self
            .matcher
            .as_ref()
            .map(|m| m.matches(data))
            .unwrap_or(true)
        {
            let new_name = self
                .transforms
                .as_ref()
                .map(|t| t.transform_name(data.name))
                .unwrap_or_else(|| Cow::Borrowed(data.name));
            Some((new_name, &self.recv))
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub enum CompiledEntrySpec<'w> {
    Concat(ConcatEntry<'w>),
    Extract(ExtractEntry<'w>),
}

impl<'w> CompiledEntrySpec<'w> {
    pub fn try_match_and_transform<'a>(
        &self,
        data: &EntryData<'a>,
    ) -> Option<MatchingEntrySpec<'a, '_, 'w>> {
        match self {
            Self::Concat(c) => c.do_match(data).map(MatchingEntrySpec::Concat),
            Self::Extract(e) => e
                .do_match_and_transform(data)
                .map(|(n, p)| MatchingEntrySpec::Extract(n, p)),
        }
    }
}

pub enum MatchingEntrySpec<'a, 'c, 'w> {
    Concat(&'c Rc<RefCell<dyn Write + 'w>>),
    Extract(Cow<'a, str>, &'c Rc<dyn EntryReceiver + 'w>),
}

impl<'a, 'c, 'w> MatchingEntrySpec<'a, 'c, 'w> {
    /* Split output handles for concat, and split generated handles by extract source and
     * name. use ptr::eq() to split, and Cow::<'s, str>::eq() with str AsRef. */
    pub fn is_nested_duplicate(
        self,
        deduped_concat_writers: &mut Vec<&'c Rc<RefCell<dyn Write + 'w>>>,
        deduped_matching_extracts: &mut Vec<(&'c Rc<dyn EntryReceiver + 'w>, Vec<Cow<'a, str>>)>,
    ) -> bool {
        match self {
            MatchingEntrySpec::Concat(concat_writer) => {
                if deduped_concat_writers
                    .iter()
                    .any(|p| Rc::ptr_eq(p, &concat_writer))
                {
                    true
                } else {
                    deduped_concat_writers.push(concat_writer);
                    false
                }
            }
            MatchingEntrySpec::Extract(name, extract_receiver) => {
                if let Some((_, names)) = deduped_matching_extracts
                    .iter_mut()
                    .find(|(p, _)| Rc::ptr_eq(p, &extract_receiver))
                {
                    if !names.iter().any(|n| n.as_ref() == name.as_ref()) {
                        names.push(name);
                        false
                    } else {
                        true
                    }
                } else {
                    deduped_matching_extracts.push((extract_receiver, vec![name]));
                    false
                }
            }
        }
    }
}

struct ParsedNamedOutputs<'w> {
    concats: HashMap<OutputName, Rc<RefCell<dyn Write + 'w>>>,
    extracts: HashMap<OutputName, Rc<dyn EntryReceiver + 'w>>,
}

pub fn process_entry_and_output_specs<'w>(
    err: Rc<RefCell<impl Write + 'w>>,
    entry_specs: impl IntoIterator<Item = EntrySpec>,
    output_specs: OutputSpecs,
) -> Result<Vec<CompiledEntrySpec<'w>>, CommandError> {
    let mut entry_specs: Vec<ParsedEntrySpecArg> = entry_specs
        .into_iter()
        .map(ParsedEntrySpecArg::from_entry_spec)
        .collect::<Result<_, _>>()?;
    if entry_specs.is_empty() {
        entry_specs.push(ParsedEntrySpecArg {
            matcher: None,
            transforms: None,
            output_name: OutputName::default_name(),
        });
    }
    let parsed_outputs = ParsedNamedOutputs::from_output_specs(err, output_specs)?;
    parsed_outputs.process_entry_specs_for_outputs(entry_specs)
}

impl<'w> ParsedNamedOutputs<'w> {
    pub fn process_entry_specs_for_outputs(
        self,
        args: impl IntoIterator<Item = ParsedEntrySpecArg>,
    ) -> Result<Vec<CompiledEntrySpec<'w>>, CommandError> {
        args.into_iter()
            .map(|arg| self.lookup_entry_spec_arg(arg))
            .collect()
    }

    fn lookup_entry_spec_arg(
        &self,
        arg: ParsedEntrySpecArg,
    ) -> Result<CompiledEntrySpec<'w>, CommandError> {
        let ParsedEntrySpecArg {
            matcher,
            transforms,
            output_name,
        } = arg;
        if let Some(stream) = self.concats.get(&output_name) {
            if transforms.is_some() {
                return Err(CommandError::InvalidArg(format!(
                    "entry name transforms do not apply to concat output {output_name:?}"
                )));
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
        concats: &mut HashMap<OutputName, Rc<RefCell<dyn Write + 'w>>>,
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
        assert!(!concats.contains_key(&name));

        let handle: Rc<RefCell<dyn Write + 'w>> = Rc::new(RefCell::new(io::stdout()));

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
        concats: &mut HashMap<OutputName, Rc<RefCell<dyn Write + 'w>>>,
    ) -> Result<(), CommandError> {
        if seen_names.contains(&name) {
            return Err(CommandError::InvalidArg(format!(
                "output name {name:?} provided more than once"
            )));
        }
        assert!(!concats.contains_key(&name));

        let handle: Rc<RefCell<dyn Write + 'w>> = {
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

        let canon_path = path
            .canonicalize()
            .wrap_err_with(|| format!("canonicalizing path {path:?} failed"))?;
        if seen_files.contains(&canon_path) {
            return Err(CommandError::InvalidArg(format!(
                "canonical output file path {canon_path:?} provided more than once"
            )));
        }

        assert!(seen_files.insert(canon_path));
        assert!(seen_names.insert(name.clone()));
        assert!(concats.insert(name, handle).is_none());
        Ok(())
    }

    fn add_dir(
        err: Rc<RefCell<impl Write + 'w>>,
        output_dir: PathBuf,
        mkdir: bool,
        name: OutputName,
        seen_dirs: &mut HashSet<PathBuf>,
        seen_names: &mut HashSet<OutputName>,
        extracts: &mut HashMap<OutputName, Rc<dyn EntryReceiver + 'w>>,
    ) -> Result<(), CommandError> {
        if seen_names.contains(&name) {
            return Err(CommandError::InvalidArg(format!(
                "output name {name:?} provided more than once"
            )));
        }
        assert!(!extracts.contains_key(&name));

        if mkdir {
            fs::create_dir_all(&output_dir)
                .wrap_err_with(|| format!("failed to create output directory {output_dir:?}"))?;
        };

        let canon_path = output_dir
            .canonicalize()
            .wrap_err_with(|| format!("canonicalizing dir path {output_dir:?} failed"))?;
        if seen_dirs.contains(&canon_path) {
            return Err(CommandError::InvalidArg(format!(
                "canonical output dir path {canon_path:?} provided more than once"
            )));
        }

        let handle: Rc<dyn EntryReceiver + 'w> = {
            let d = FilesystemReceiver::new(err, output_dir);
            Rc::new(d)
        };

        assert!(seen_dirs.insert(canon_path));
        assert!(seen_names.insert(name.clone()));
        assert!(extracts.insert(name, handle).is_none());
        Ok(())
    }

    pub fn from_output_specs(
        err: Rc<RefCell<impl Write + 'w>>,
        spec: OutputSpecs,
    ) -> Result<Self, CommandError> {
        let OutputSpecs { default, named } = spec;

        let mut concats: HashMap<OutputName, Rc<RefCell<dyn Write + 'w>>> = HashMap::new();
        let mut extracts: HashMap<OutputName, Rc<dyn EntryReceiver + 'w>> = HashMap::new();

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
                        err.clone(),
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
            let name = OutputName(name);
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
                        err.clone(),
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

pub trait EntryReceiver: fmt::Debug {
    fn generate_entry_handle<'s>(
        &self,
        data: EntryData<'s>,
        symlink_target: Option<&[u8]>,
        name: Cow<'s, str>,
    ) -> Result<Option<Box<dyn Write>>, CommandError>;

    fn finalize_entries(&self) -> Result<(), CommandError>;
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg(unix)]
struct PermsEntry {
    path: PathBuf,
    mode: u32,
}

struct FilesystemReceiver<W> {
    err: Rc<RefCell<W>>,
    output_dir: PathBuf,
    #[cfg(unix)]
    perms_to_set: RefCell<Vec<PermsEntry>>,
}

impl<W> FilesystemReceiver<W> {
    pub fn new(err: Rc<RefCell<W>>, output_dir: PathBuf) -> Self {
        Self {
            err,
            output_dir,
            #[cfg(unix)]
            perms_to_set: RefCell::new(Vec::new()),
        }
    }
}

impl<W> fmt::Debug for FilesystemReceiver<W> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "FilesystemReceiver {{ output_dir: {:?} }}",
            &self.output_dir
        )
    }
}

impl<W> EntryReceiver for FilesystemReceiver<W>
where
    W: Write,
{
    fn generate_entry_handle<'s>(
        &self,
        data: EntryData<'s>,
        symlink_target: Option<&[u8]>,
        name: Cow<'s, str>,
    ) -> Result<Option<Box<dyn Write>>, CommandError> {
        let mut err = self.err.borrow_mut();
        let full_output_path = self.output_dir.join(name.as_ref());
        writeln!(
            err,
            "receiving entry {} with name {name} and writing to path {full_output_path:?}",
            data.name
        )
        .unwrap();

        match data.kind {
            EntryKind::Dir => {
                writeln!(err, "entry is directory, creating").unwrap();
                fs::create_dir_all(&full_output_path).wrap_err_with(|| {
                    format!("failed to create directory entry at {full_output_path:?}")
                })?;
            }
            EntryKind::Symlink => {
                let target: Vec<u8> = symlink_target
                    .expect("we should have generated this")
                    .to_vec();

                #[cfg(unix)]
                {
                    use std::{
                        ffi::OsString,
                        os::unix::{ffi::OsStringExt, fs::symlink},
                    };
                    let target = OsString::from_vec(target);
                    writeln!(err, "entry is symlink to {target:?}, creating").unwrap();
                    /* The stdlib symlink function has no functionality like OpenOptions to
                     * truncate a symlink if it already exists, so we have to do that ourselves
                     * here. */
                    if let Err(e) = symlink(&target, &full_output_path) {
                        let e = match e.kind() {
                            io::ErrorKind::AlreadyExists => {
                                writeln!(err, "a file already existed at the symlink target {full_output_path:?}, removing")
                                    .unwrap();
                                fs::remove_file(&full_output_path)
                                    .wrap_err_with(|| format!("failed to remove file at symlink target {full_output_path:?}"))?;
                                writeln!(
                                    err,
                                    "successfully removed file entry, creating symlink again"
                                )
                                .unwrap();
                                symlink(&target, &full_output_path).err()
                            }
                            _ => Some(e),
                        };
                        if let Some(e) = e {
                            return Err(e).wrap_err_with(|| {
                                format!(
                                    "failed to create symlink at {full_output_path:?} with target {target:?}"
                                )
                            });
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    todo!("TODO: cannot create symlink for entry {name} on non-unix yet!");
                }
            }
            EntryKind::File => {
                writeln!(err, "entry is file, creating").unwrap();
                if let Some(containing_dir) = full_output_path.parent() {
                    fs::create_dir_all(containing_dir).wrap_err_with(|| {
                        format!("failed to create parent dirs for file at {full_output_path:?}")
                    })?;
                } else {
                    writeln!(err, "entry had no parent dir (in root dir?)").unwrap();
                }
                let outfile = fs::File::create(&full_output_path)
                    .wrap_err_with(|| format!("failed to create file at {full_output_path:?}"))?;
                return Ok(Some(Box::new(outfile)));
            }
        }

        #[cfg(unix)]
        if let Some(mode) = data.unix_mode {
            writeln!(
                err,
                "storing unix mode {mode} for path {full_output_path:?}"
            )
            .unwrap();
            self.perms_to_set.borrow_mut().push(PermsEntry {
                path: full_output_path,
                mode,
            });
        }

        Ok(None)
    }

    fn finalize_entries(&self) -> Result<(), CommandError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut perms_to_set = mem::take(&mut *self.perms_to_set.borrow_mut());
            perms_to_set.sort_unstable();
            writeln!(
                &mut self.err.borrow_mut(),
                "perms to set (these are done in reverse order): {perms_to_set:?}"
            )
            .unwrap();
            for PermsEntry { path, mode } in perms_to_set.into_iter().rev() {
                let perms = fs::Permissions::from_mode(mode);
                fs::set_permissions(&path, perms.clone())
                    .wrap_err_with(|| format!("error setting perms {perms:?} for path {path:?}"))?;
            }
        }
        Ok(())
    }
}
