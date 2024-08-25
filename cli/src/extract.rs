use std::{
    borrow::Cow,
    cell::RefCell,
    collections::VecDeque,
    env, fs,
    io::{self, Read, Write},
    mem,
    path::{Path, PathBuf},
    rc::Rc,
};

use zip::{
    read::{read_zipfile_from_stream, ZipFile},
    ZipArchive,
};

use crate::{args::extract::*, CommandError, WrapCommandErr};

trait EntryReceiver {
    fn receive_entry(&self, entry: ZipFile, name: &str) -> Result<(), CommandError>;
    fn finalize_entries(&self) -> Result<(), CommandError>;
}

fn make_entry_receiver<'a>(
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
    fn receive_entry(&self, mut entry: ZipFile, name: &str) -> Result<(), CommandError> {
        let mut err = self.err.borrow_mut();
        writeln!(err, "receiving entry {} with name {name}", entry.name()).unwrap();
        if entry.is_dir() {
            writeln!(err, "entry is directory, ignoring").unwrap();
        } else if entry.is_symlink() {
            writeln!(err, "entry is symlink, ignoring").unwrap();
        } else {
            io::copy(&mut entry, &mut self.stdout.lock())
                .wrap_err_with(|| format!("failed to write entry {name} to stdout"))?;
        }
        Ok(())
    }

    fn finalize_entries(&self) -> Result<(), CommandError> {
        Ok(())
    }
}

struct FilesystemReceiver<W> {
    err: Rc<RefCell<W>>,
    output_dir: PathBuf,
    #[cfg(unix)]
    perms_to_set: RefCell<Vec<(PathBuf, u32)>>,
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

impl<W> EntryReceiver for FilesystemReceiver<W>
where
    W: Write,
{
    fn receive_entry(&self, mut entry: ZipFile, name: &str) -> Result<(), CommandError> {
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
            self.perms_to_set
                .borrow_mut()
                .push((full_output_path.clone(), mode));
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
            io::copy(&mut entry, &mut outfile).wrap_err_with(|| {
                format!(
                    "failed to copy file contents from {} to {full_output_path:?}",
                    entry.name()
                )
            })?;
        }
        Ok(())
    }

    fn finalize_entries(&self) -> Result<(), CommandError> {
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

struct Matcher<W> {
    err: Rc<RefCell<W>>,
    expr: MatchExpression,
}

impl<W> Matcher<W> {
    pub fn new(err: Rc<RefCell<W>>, expr: MatchExpression) -> Self {
        Self { err, expr }
    }
}

impl<W> Matcher<W>
where
    W: Write,
{
    pub fn evaluate(&self, entry: &ZipFile) -> Result<bool, CommandError> {
        let Self { err, expr } = self;
        Self::recursive_match(err, &expr, entry)
    }

    fn recursive_match(
        err: &RefCell<W>,
        expr: &MatchExpression,
        entry: &ZipFile,
    ) -> Result<bool, CommandError> {
        match expr {
            MatchExpression::PrimitivePredicate(predicate) => match predicate {
                Predicate::Trivial(trivial) => match trivial {
                    TrivialPredicate::True => Ok(true),
                    TrivialPredicate::False => Ok(false),
                },
                Predicate::EntryType(entry_type) => match entry_type {
                    EntryType::File => Ok(!entry.is_dir() && !entry.is_symlink()),
                    EntryType::Dir => Ok(entry.is_dir()),
                    EntryType::Symlink => Ok(entry.is_symlink()),
                },
                Predicate::CompressionMethod(method_arg) => match method_arg {
                    CompressionMethodArg::NonSpecific(nonspecific_arg) => match nonspecific_arg {
                        NonSpecificCompressionMethodArg::Any => Ok(true),
                        NonSpecificCompressionMethodArg::Known => {
                            Ok(SpecificCompressionMethodArg::KNOWN_COMPRESSION_METHODS
                                .contains(&entry.compression()))
                        }
                    },
                    CompressionMethodArg::Specific(specific_arg) => {
                        Ok(specific_arg.translate_to_zip() == entry.compression())
                    }
                },
                Predicate::DepthLimit(limit_arg) => match limit_arg {
                    DepthLimitArg::Max(max) => {
                        let max: usize = (*max).into();
                        Ok(entry.name().split('/').count() <= max)
                    }
                    DepthLimitArg::Min(min) => {
                        let min: usize = (*min).into();
                        Ok(entry.name().split('/').count() >= min)
                    }
                },
                Predicate::Match(match_arg) => todo!("{match_arg:?}"),
            },
            MatchExpression::Negated(inner) => {
                Self::recursive_match(err, inner.as_ref(), entry).map(|result| !result)
            }
            MatchExpression::And {
                explicit: _,
                left,
                right,
            } => {
                /* Short-circuiting, so do left first. */
                Ok(Self::recursive_match(err, left.as_ref(), entry)?
                    && Self::recursive_match(err, right.as_ref(), entry)?)
            }
            MatchExpression::Or { left, right } => {
                Ok(Self::recursive_match(err, left.as_ref(), entry)?
                    || Self::recursive_match(err, right.as_ref(), entry)?)
            }
            MatchExpression::Grouped(inner) => Self::recursive_match(err, inner.as_ref(), entry),
        }
    }
}

struct Transformer<W> {
    err: Rc<RefCell<W>>,
    trans: NameTransform,
}

impl<W> Transformer<W> {
    pub fn new(err: Rc<RefCell<W>>, trans: NameTransform) -> Self {
        Self { err, trans }
    }
}

impl<W> Transformer<W>
where
    W: Write,
{
    pub fn evaluate<'s>(&self, name: &'s str) -> Result<Cow<'s, str>, CommandError> {
        match &self.trans {
            NameTransform::Trivial(TrivialTransform::Identity) => Ok(Cow::Borrowed(name)),
            NameTransform::Basic(basic_trans) => match basic_trans {
                BasicTransform::StripComponents(num_components_to_strip) => {
                    /* If no directory components, then nothing to strip. */
                    if !name.contains('/') {
                        return Ok(Cow::Borrowed(name));
                    }
                    /* We allow stripping 0 components, which does nothing. */
                    if *num_components_to_strip == 0 {
                        return Ok(Cow::Borrowed(name));
                    }
                    /* Pop off prefix components until only one is left or we have stripped all the
                     * requested prefix components. */
                    let mut num_components_to_strip: usize = (*num_components_to_strip).into();
                    let mut separator_indices: VecDeque<usize> =
                        name.match_indices('/').map(|(i, _)| i).collect();
                    debug_assert!(separator_indices.len() > 0);
                    /* Always keep the final separator, as regardless of how many we strip, we want
                     * to keep the basename in all cases. */
                    while separator_indices.len() > 1 && num_components_to_strip > 0 {
                        let _ = separator_indices.pop_front().unwrap();
                        num_components_to_strip -= 1;
                    }
                    debug_assert!(separator_indices.len() > 0);
                    let leftmost_remaining_separator_index: usize =
                        separator_indices.pop_front().unwrap();
                    Ok(Cow::Borrowed(
                        &name[(leftmost_remaining_separator_index + 1)..],
                    ))
                }
                BasicTransform::AddPrefix(prefix_to_add) => {
                    /* We allow an empty prefix, which means to do nothing. */
                    if prefix_to_add.is_empty() {
                        return Ok(Cow::Borrowed(name));
                    }
                    Ok(Cow::Owned(format!("{}/{}", prefix_to_add, name)))
                }
            },
            NameTransform::Complex(complex_trans) => match complex_trans {
                ComplexTransform::RemovePrefix(remove_prefix_arg) => {
                    todo!("impl remove prefix: {:?}", remove_prefix_arg)
                }
                ComplexTransform::Transform(transform_arg) => {
                    todo!("impl transform: {:?}", transform_arg)
                }
            },
        }
    }
}

enum EntryContent<'a> {
    Decompressed(ZipFile<'a>),
    /* See ContentTransform::Raw -- need to refactor this file to avoid the need to convert
     * a ZipFile into a Raw after it's already constructed. */
    #[allow(dead_code)]
    Raw(ZipFile<'a>),
    LogToStderr(ZipFile<'a>),
}

struct ContentTransformer<W> {
    err: Rc<RefCell<W>>,
    arg: ContentTransform,
}

impl<W> ContentTransformer<W> {
    pub fn new(err: Rc<RefCell<W>>, arg: ContentTransform) -> Self {
        Self { err, arg }
    }
}

impl<W> ContentTransformer<W>
where
    W: Write,
{
    pub fn transform_matched_entry<'a>(&self, entry: ZipFile<'a>) -> EntryContent<'a> {
        match self.arg {
            ContentTransform::Extract => EntryContent::Decompressed(entry),
            ContentTransform::Raw => unreachable!("this has not been implemented"),
            ContentTransform::LogToStderr => EntryContent::LogToStderr(entry),
        }
    }
}

struct EntrySpecTransformer<W> {
    err: Rc<RefCell<W>>,
    matcher: Option<Matcher<W>>,
    name_transformers: Vec<Transformer<W>>,
    content: ContentTransformer<W>,
}

impl<W> EntrySpecTransformer<W> {
    pub fn new(
        err: Rc<RefCell<W>>,
        match_expr: Option<MatchExpression>,
        name_transforms: impl IntoIterator<Item = NameTransform>,
        content: ContentTransform,
    ) -> Self {
        let matcher = match_expr.map(|expr| Matcher::new(err.clone(), expr));
        let name_transformers: Vec<_> = name_transforms
            .into_iter()
            .map(|trans| Transformer::new(err.clone(), trans))
            .collect();
        let content = ContentTransformer::new(err.clone(), content);
        Self {
            err,
            matcher,
            name_transformers,
            content,
        }
    }

    pub fn empty(err: Rc<RefCell<W>>) -> Self {
        let content = ContentTransformer::new(err.clone(), ContentTransform::Extract);
        Self {
            err,
            matcher: None,
            name_transformers: Vec::new(),
            content,
        }
    }
}

trait IterateEntries {
    fn next_entry(&mut self) -> Result<Option<ZipFile>, CommandError>;
}

struct StdinInput<W> {
    err: Rc<RefCell<W>>,
    inner: io::Stdin,
}

impl<W> StdinInput<W> {
    pub fn new(err: Rc<RefCell<W>>) -> Self {
        Self {
            err,
            inner: io::stdin(),
        }
    }
}

impl<W> IterateEntries for StdinInput<W> {
    fn next_entry(&mut self) -> Result<Option<ZipFile>, CommandError> {
        read_zipfile_from_stream(&mut self.inner).wrap_err("failed to read zip entries from stdin")
    }
}

#[derive(Debug)]
struct ZipFileInput<W> {
    err: Rc<RefCell<W>>,
    inner: ZipArchive<fs::File>,
    file_counter: usize,
}

impl<W> ZipFileInput<W> {
    pub fn new(err: Rc<RefCell<W>>, inner: ZipArchive<fs::File>) -> Self {
        Self {
            err,
            inner: inner,
            file_counter: 0,
        }
    }

    pub fn remaining(&self) -> usize {
        self.inner.len() - self.file_counter
    }

    pub fn none_left(&self) -> bool {
        self.remaining() == 0
    }
}

impl<W> IterateEntries for ZipFileInput<W> {
    fn next_entry(&mut self) -> Result<Option<ZipFile>, CommandError> {
        if self.none_left() {
            return Ok(None);
        }
        let prev_counter = self.file_counter;
        self.file_counter += 1;
        self.inner
            .by_index(prev_counter)
            .map(Some)
            .wrap_err_with(|| format!("failed to read entry #{prev_counter} from zip",))
    }
}

struct AllInputZips<W> {
    err: Rc<RefCell<W>>,
    zips_todo: Vec<ZipFileInput<W>>,
}

impl<W> AllInputZips<W> {
    pub fn new(
        err: Rc<RefCell<W>>,
        zip_paths: impl IntoIterator<Item = impl AsRef<Path>>,
    ) -> Result<Self, CommandError> {
        let zips_todo = zip_paths
            .into_iter()
            .map(|p| {
                fs::File::open(p.as_ref())
                    .wrap_err_with(|| {
                        format!("failed to open zip input file path {:?}", p.as_ref())
                    })
                    .and_then(|f| {
                        ZipArchive::new(f).wrap_err_with(|| {
                            format!("failed to create zip archive for file {:?}", p.as_ref())
                        })
                    })
                    .map(|archive| ZipFileInput::new(Rc::clone(&err), archive))
            })
            .collect::<Result<Vec<_>, CommandError>>()?;
        Ok(Self { err, zips_todo })
    }

    pub fn iter_zips(self) -> impl IntoIterator<Item = ZipFileInput<W>> {
        self.zips_todo.into_iter()
    }
}

pub fn execute_extract(mut err: impl Write, extract: Extract) -> Result<(), CommandError> {
    writeln!(err, "asdf!").unwrap();

    dbg!(extract);

    Ok(())
}
