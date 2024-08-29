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
