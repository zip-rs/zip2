use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    fs,
    io::{self, Seek, Write},
    path::PathBuf,
    rc::Rc,
};

use super::matcher::{CompiledMatcher, EntryMatcher};
use super::receiver::{
    CompiledEntrySpec, ConcatEntry, EntryReceiver, ExtractEntry, FilesystemReceiver,
};
use super::transform::{CompiledTransformer, NameTransformer};
use crate::{args::extract::*, CommandError, WrapCommandErr};

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

struct NamedOutputsBuilder<'w, W> {
    err: Rc<RefCell<W>>,
    concats: HashMap<OutputName, Rc<RefCell<dyn Write + 'w>>>,
    extracts: HashMap<OutputName, Rc<dyn EntryReceiver + 'w>>,
    seen_stdout: bool,
    seen_files: HashSet<PathBuf>,
    seen_dirs: HashSet<PathBuf>,
    seen_names: HashSet<OutputName>,
}

impl<'w, W> NamedOutputsBuilder<'w, W> {
    pub fn new(err: Rc<RefCell<W>>) -> Self {
        Self {
            err,
            concats: HashMap::new(),
            extracts: HashMap::new(),
            seen_stdout: false,
            seen_files: HashSet::new(),
            seen_dirs: HashSet::new(),
            seen_names: HashSet::new(),
        }
    }

    pub fn into_tables(
        self,
    ) -> (
        HashMap<OutputName, Rc<RefCell<dyn Write + 'w>>>,
        HashMap<OutputName, Rc<dyn EntryReceiver + 'w>>,
    ) {
        let Self {
            concats, extracts, ..
        } = self;
        (concats, extracts)
    }

    fn add_name<T>(
        &mut self,
        name: OutputName,
        f: impl FnOnce() -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        if self.seen_names.contains(&name) {
            return Err(CommandError::InvalidArg(format!(
                "output name {name:?} provided more than once"
            )));
        }

        let ret = f()?;

        assert!(self.seen_names.insert(name));

        Ok(ret)
    }

    fn add_concat(
        &mut self,
        name: OutputName,
        handle: impl Write + 'w,
    ) -> Result<(), CommandError> {
        /* This should be assured by the check against self.seen_names. */
        assert!(!self.concats.contains_key(&name));

        let handle = Rc::new(RefCell::new(handle));

        assert!(self.concats.insert(name, handle).is_none());

        Ok(())
    }

    pub fn add_stdout(&mut self, name: OutputName) -> Result<(), CommandError> {
        if self.seen_stdout {
            return Err(CommandError::InvalidArg(
                "--stdout output provided for more than one receiver".to_string(),
            ));
        }

        let handle = self.add_name(name.clone(), || Ok(io::stdout()))?;
        self.add_concat(name, handle)?;

        self.seen_stdout = true;
        Ok(())
    }

    fn add_seen_file(&mut self, path: PathBuf) -> Result<(), CommandError> {
        let canon_path = path
            .canonicalize()
            .wrap_err_with(|| format!("canonicalizing path {path:?} failed"))?;

        if self.seen_files.contains(&canon_path) {
            return Err(CommandError::InvalidArg(format!(
                "canonical output file path {canon_path:?} provided more than once"
            )));
        }

        assert!(self.seen_files.insert(canon_path));

        Ok(())
    }

    pub fn add_file(
        &mut self,
        path: PathBuf,
        append: bool,
        name: OutputName,
    ) -> Result<(), CommandError> {
        let handle = self.add_name(name.clone(), || {
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
            Ok(f)
        })?;
        self.add_seen_file(path)?;
        self.add_concat(name, handle)?;
        Ok(())
    }

    fn add_seen_dir(&mut self, path: PathBuf) -> Result<(), CommandError> {
        let canon_path = path
            .canonicalize()
            .wrap_err_with(|| format!("canonicalizing dir path {path:?} failed"))?;
        if self.seen_dirs.contains(&canon_path) {
            return Err(CommandError::InvalidArg(format!(
                "canonical output dir path {canon_path:?} provided more than once"
            )));
        }

        assert!(self.seen_dirs.insert(canon_path));

        Ok(())
    }

    fn add_extract(
        &mut self,
        name: OutputName,
        handle: impl EntryReceiver + 'w,
    ) -> Result<(), CommandError> {
        assert!(!self.extracts.contains_key(&name));

        let handle = Rc::new(handle);

        assert!(self.extracts.insert(name, handle).is_none());

        Ok(())
    }
}

impl<'w, W> NamedOutputsBuilder<'w, W>
where
    W: Write + 'w,
{
    pub fn add_dir(
        &mut self,
        output_dir: PathBuf,
        mkdir: bool,
        name: OutputName,
    ) -> Result<(), CommandError> {
        let err = self.err.clone();
        let handle = self.add_name(name.clone(), || {
            if mkdir {
                fs::create_dir_all(&output_dir).wrap_err_with(|| {
                    format!("failed to create output directory {output_dir:?}")
                })?;
            };
            Ok(FilesystemReceiver::new(err, output_dir.clone()))
        })?;
        self.add_seen_dir(output_dir.clone())?;
        self.add_extract(name, handle)?;
        Ok(())
    }
}

struct ParsedNamedOutputs<'w> {
    concats: HashMap<OutputName, Rc<RefCell<dyn Write + 'w>>>,
    extracts: HashMap<OutputName, Rc<dyn EntryReceiver + 'w>>,
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

    pub fn from_output_specs(
        err: Rc<RefCell<impl Write + 'w>>,
        spec: OutputSpecs,
    ) -> Result<Self, CommandError> {
        let OutputSpecs { default, named } = spec;

        let mut builder = NamedOutputsBuilder::new(err);

        if let Some(default) = default {
            let name = OutputName::default_name();
            match default {
                OutputCollation::ConcatenateStdout => {
                    builder.add_stdout(name)?;
                }
                OutputCollation::ConcatenateFile { path, append } => {
                    builder.add_file(path, append, name)?;
                }
                OutputCollation::Filesystem { output_dir, mkdir } => {
                    builder.add_dir(output_dir, mkdir, name)?;
                }
            }
        }
        for NamedOutput { name, output } in named.into_iter() {
            let name = OutputName(name);
            match output {
                OutputCollation::ConcatenateStdout => {
                    builder.add_stdout(name)?;
                }
                OutputCollation::ConcatenateFile { path, append } => {
                    builder.add_file(path, append, name)?;
                }
                OutputCollation::Filesystem { output_dir, mkdir } => {
                    builder.add_dir(output_dir, mkdir, name)?;
                }
            }
        }

        let (concats, extracts) = builder.into_tables();
        Ok(Self { concats, extracts })
    }
}
