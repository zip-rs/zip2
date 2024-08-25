use std::{
    borrow::Cow,
    cell::{RefCell, UnsafeCell},
    collections::VecDeque,
    env, fs,
    io::{self, Read, Write},
    mem,
    path::{Path, PathBuf},
    rc::Rc,
};

use glob;
use regex;

use zip::{
    read::{read_zipfile_from_stream, ZipFile},
    CompressionMethod, ZipArchive,
};

use crate::{args::extract::*, CommandError, WrapCommandErr};

trait EntryReceiver {
    fn receive_entry<'a>(
        &mut self,
        entry: &mut ZipFile<'a>,
        name: &str,
    ) -> Result<(), CommandError>;
    fn finalize_entries(&mut self) -> Result<(), CommandError>;
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

#[inline(always)]
fn process_component_selector<'s>(sel: ComponentSelector, name: &'s str) -> Option<&'s str> {
    let path = Path::new(name);
    match sel {
        ComponentSelector::Path => Some(name),
        ComponentSelector::Basename => path.file_name().map(|bname| bname.to_str().unwrap()),
        ComponentSelector::Dirname => path
            .parent()
            .map(|p| p.to_str().unwrap())
            /* "a".parent() becomes Some(""), which we want to treat as no parent */
            .filter(|s| !s.is_empty()),
        ComponentSelector::FileExtension => path.extension().map(|ext| ext.to_str().unwrap()),
    }
}

trait NameMatcher {
    fn create(pattern: &str, opts: PatternModifiers) -> Result<Self, CommandError>
    where
        Self: Sized;
    fn matches(&self, input: &str) -> bool;
}

struct LiteralMatcher {
    lit: String,
    case_insensitive: bool,
}

impl NameMatcher for LiteralMatcher {
    fn create(pattern: &str, opts: PatternModifiers) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        let PatternModifiers { case_insensitive } = opts;
        Ok(Self {
            lit: pattern.to_string(),
            case_insensitive,
        })
    }

    fn matches(&self, input: &str) -> bool {
        if self.case_insensitive {
            self.lit.eq_ignore_ascii_case(input)
        } else {
            input == &self.lit
        }
    }
}

struct GlobMatcher {
    pat: glob::Pattern,
    glob_opts: glob::MatchOptions,
}

impl NameMatcher for GlobMatcher {
    fn create(pattern: &str, opts: PatternModifiers) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        let PatternModifiers { case_insensitive } = opts;
        let glob_opts = glob::MatchOptions {
            case_sensitive: !case_insensitive,
            ..Default::default()
        };
        let pat = glob::Pattern::new(pattern).map_err(|e| {
            CommandError::InvalidArg(format!(
                "failed to construct glob matcher from pattern {pattern:?}: {e}"
            ))
        })?;
        Ok(Self { pat, glob_opts })
    }

    fn matches(&self, input: &str) -> bool {
        self.pat.matches_with(input, self.glob_opts)
    }
}

struct RegexMatcher {
    pat: regex::Regex,
}

impl NameMatcher for RegexMatcher {
    fn create(pattern: &str, opts: PatternModifiers) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        let PatternModifiers { case_insensitive } = opts;
        let pat = regex::RegexBuilder::new(pattern)
            .case_insensitive(case_insensitive)
            .build()
            .map_err(|e| {
                CommandError::InvalidArg(format!(
                    "failed to construct regex matcher from pattern {pattern:?}: {e}"
                ))
            })?;
        Ok(Self { pat })
    }

    fn matches(&self, input: &str) -> bool {
        self.pat.is_match(input)
    }
}

trait EntryMatcher {
    type Arg
    where
        Self: Sized;
    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized;
    fn matches(&self, entry: &ZipFile) -> bool;
}

#[derive(Copy, Clone)]
enum TrivialMatcher {
    True,
    False,
}

impl EntryMatcher for TrivialMatcher {
    type Arg = TrivialPredicate where Self: Sized;

    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        Ok(match arg {
            TrivialPredicate::True => Self::True,
            TrivialPredicate::False => Self::False,
        })
    }

    fn matches(&self, _entry: &ZipFile) -> bool {
        match self {
            Self::True => true,
            Self::False => false,
        }
    }
}

#[derive(Copy, Clone)]
enum EntryTypeMatcher {
    File,
    Dir,
    Symlink,
}

impl EntryMatcher for EntryTypeMatcher {
    type Arg = EntryType where Self: Sized;

    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        Ok(match arg {
            EntryType::File => Self::File,
            EntryType::Dir => Self::Dir,
            EntryType::Symlink => Self::Symlink,
        })
    }

    fn matches(&self, entry: &ZipFile) -> bool {
        match self {
            Self::File => !entry.is_dir() && !entry.is_symlink(),
            Self::Dir => entry.is_dir(),
            Self::Symlink => entry.is_symlink(),
        }
    }
}

#[derive(Copy, Clone)]
enum NonSpecificMethods {
    Any,
    Known,
}

impl EntryMatcher for NonSpecificMethods {
    type Arg = NonSpecificCompressionMethodArg where Self: Sized;

    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        Ok(match arg {
            NonSpecificCompressionMethodArg::Any => Self::Any,
            NonSpecificCompressionMethodArg::Known => Self::Known,
        })
    }

    fn matches(&self, entry: &ZipFile) -> bool {
        match self {
            Self::Any => true,
            Self::Known => SpecificCompressionMethodArg::KNOWN_COMPRESSION_METHODS
                .contains(&entry.compression()),
        }
    }
}

struct SpecificMethods {
    specific_method: CompressionMethod,
}

impl EntryMatcher for SpecificMethods {
    type Arg = SpecificCompressionMethodArg where Self: Sized;

    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        Ok(Self {
            specific_method: arg.translate_to_zip(),
        })
    }

    fn matches(&self, entry: &ZipFile) -> bool {
        self.specific_method == entry.compression()
    }
}

#[derive(Copy, Clone)]
enum DepthLimit {
    Max(usize),
    Min(usize),
}

impl EntryMatcher for DepthLimit {
    type Arg = DepthLimitArg where Self: Sized;

    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        Ok(match arg {
            DepthLimitArg::Max(max) => Self::Max(max.into()),
            DepthLimitArg::Min(min) => Self::Min(min.into()),
        })
    }

    fn matches(&self, entry: &ZipFile) -> bool {
        let num_components = entry.name().split('/').count();
        match self {
            Self::Max(max) => num_components <= *max,
            Self::Min(min) => num_components >= *min,
        }
    }
}

struct PatternMatcher {
    matcher: Box<dyn NameMatcher>,
    comp_sel: ComponentSelector,
}

impl EntryMatcher for PatternMatcher {
    type Arg = MatchArg where Self: Sized;

    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        let MatchArg {
            comp_sel,
            pat_sel: PatternSelector { pat_sel, modifiers },
            pattern,
        } = arg;

        let matcher: Box<dyn NameMatcher> = match pat_sel {
            PatternSelectorType::Glob => Box::new(GlobMatcher::create(&pattern, modifiers)?),
            PatternSelectorType::Literal => Box::new(LiteralMatcher::create(&pattern, modifiers)?),
            PatternSelectorType::Regexp => Box::new(RegexMatcher::create(&pattern, modifiers)?),
        };

        Ok(Self { matcher, comp_sel })
    }

    fn matches(&self, entry: &ZipFile) -> bool {
        match process_component_selector(self.comp_sel, entry.name()) {
            None => false,
            Some(s) => self.matcher.matches(s),
        }
    }
}

enum WrappedMatcher {
    Primitive(Box<dyn EntryMatcher>),
    Negated(Box<dyn EntryMatcher>),
    And {
        left: Box<dyn EntryMatcher>,
        right: Box<dyn EntryMatcher>,
    },
    Or {
        left: Box<dyn EntryMatcher>,
        right: Box<dyn EntryMatcher>,
    },
}

impl WrappedMatcher {
    fn create_primitive(arg: Predicate) -> Result<Self, CommandError> {
        Ok(Self::Primitive(match arg {
            Predicate::Trivial(arg) => Box::new(TrivialMatcher::from_arg(arg)?),
            Predicate::EntryType(arg) => Box::new(EntryTypeMatcher::from_arg(arg)?),
            Predicate::CompressionMethod(method_arg) => match method_arg {
                CompressionMethodArg::NonSpecific(arg) => {
                    Box::new(NonSpecificMethods::from_arg(arg)?)
                }
                CompressionMethodArg::Specific(arg) => Box::new(SpecificMethods::from_arg(arg)?),
            },
            Predicate::DepthLimit(arg) => Box::new(DepthLimit::from_arg(arg)?),
            Predicate::Match(arg) => Box::new(PatternMatcher::from_arg(arg)?),
        }))
    }
}

impl EntryMatcher for WrappedMatcher {
    type Arg = MatchExpression where Self: Sized;

    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        Ok(match arg {
            MatchExpression::PrimitivePredicate(pred) => Self::create_primitive(pred)?,
            MatchExpression::Negated(arg) => Self::Negated(Box::new(Self::from_arg(*arg)?)),
            MatchExpression::And {
                explicit: _,
                left,
                right,
            } => {
                let left = Box::new(Self::from_arg(*left)?);
                let right = Box::new(Self::from_arg(*right)?);
                Self::And { left, right }
            }
            MatchExpression::Or { left, right } => {
                let left = Box::new(Self::from_arg(*left)?);
                let right = Box::new(Self::from_arg(*right)?);
                Self::Or { left, right }
            }
            MatchExpression::Grouped(inner) => Self::from_arg(*inner)?,
        })
    }

    fn matches(&self, entry: &ZipFile) -> bool {
        match self {
            Self::Primitive(m) => m.matches(entry),
            Self::Negated(m) => !m.matches(entry),
            Self::And { left, right } => left.matches(entry) && right.matches(entry),
            Self::Or { left, right } => left.matches(entry) || right.matches(entry),
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
    pub fn evaluate<'s>(&self, name: &'s str) -> Cow<'s, str> {
        match &self.trans {
            NameTransform::Trivial(TrivialTransform::Identity) => Cow::Borrowed(name),
            NameTransform::Basic(basic_trans) => match basic_trans {
                BasicTransform::StripComponents(num_components_to_strip) => {
                    /* If no directory components, then nothing to strip. */
                    if !name.contains('/') {
                        return Cow::Borrowed(name);
                    }
                    /* We allow stripping 0 components, which does nothing. */
                    if *num_components_to_strip == 0 {
                        return Cow::Borrowed(name);
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
                    Cow::Borrowed(&name[(leftmost_remaining_separator_index + 1)..])
                }
                BasicTransform::AddPrefix(prefix_to_add) => {
                    /* We allow an empty prefix, which means to do nothing. */
                    if prefix_to_add.is_empty() {
                        return Cow::Borrowed(name);
                    }
                    Cow::Owned(format!("{}/{}", prefix_to_add, name))
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

struct EntrySpecTransformer<W> {
    err: Rc<RefCell<W>>,
    matcher: Option<WrappedMatcher>,
    name_transformers: Vec<Transformer<W>>,
    content_transform: ContentTransform,
}

impl<W> EntrySpecTransformer<W> {
    pub fn new(err: Rc<RefCell<W>>, entry_spec: EntrySpec) -> Result<Self, CommandError> {
        let EntrySpec {
            match_expr,
            name_transforms,
            content_transform,
        } = entry_spec;
        let matcher = match match_expr {
            None => None,
            Some(expr) => Some(WrappedMatcher::from_arg(expr)?),
        };
        let name_transformers: Vec<_> = name_transforms
            .into_iter()
            .map(|trans| Transformer::new(err.clone(), trans))
            .collect();
        Ok(Self {
            err,
            matcher,
            name_transformers,
            content_transform,
        })
    }

    pub fn empty(err: Rc<RefCell<W>>) -> Self {
        Self {
            err,
            matcher: None,
            name_transformers: Vec::new(),
            content_transform: ContentTransform::Extract,
        }
    }
}

impl<W> EntrySpecTransformer<W>
where
    W: Write,
{
    pub fn matches(&self, entry: &ZipFile) -> bool {
        match &self.matcher {
            None => true,
            Some(matcher) => matcher.matches(entry),
        }
    }

    /// Transform the name from the zip entry, maintaining a few invariants:
    /// 1. If the transformations all return substrings (no prefixing, non-empty replacements, or
    ///    empty replacements that lead to non-contiguous input chunks), return a slice of the
    ///    original input, pointing back to the ZipFile's memory location with associated lifetime.
    /// 2. If some intermediate transformation requires an allocation (e.g. adding a prefix), do
    ///    not perform intermediate reallocations for subsequent substring-only transformations.
    ///    - TODO: The returned string may be reallocated from the initial allocation exactly once
    ///      at the end, if substring-only transformations reduced its length. This is because Cow
    ///      can only describe a substring of the original input or an entirely new allocated
    ///      string, as opposed to a more general sort of string view wrapper.
    pub fn transform_name<'s>(&self, mut original_name: &'s str) -> Cow<'s, str> {
        let mut newly_allocated_name: Option<String> = None;
        let mut newly_allocated_str: Option<&str> = None;
        for transformer in self.name_transformers.iter() {
            match newly_allocated_str {
                Some(s) => match transformer.evaluate(s) {
                    Cow::Borrowed(t) => {
                        let _ = newly_allocated_str.replace(t);
                    }
                    Cow::Owned(t) => {
                        assert!(newly_allocated_name.replace(t).is_some());
                        newly_allocated_str = Some(newly_allocated_name.as_ref().unwrap().as_str());
                    }
                },
                None => match transformer.evaluate(original_name) {
                    Cow::Borrowed(t) => {
                        original_name = t;
                    }
                    Cow::Owned(t) => {
                        assert!(newly_allocated_name.replace(t).is_none());
                        newly_allocated_str = Some(newly_allocated_name.as_ref().unwrap().as_str());
                    }
                },
            }
        }

        if newly_allocated_name.is_none() {
            /* If we have never allocated anything new, just return the substring of the original
             * name! */
            Cow::Borrowed(original_name)
        } else {
            let subref = newly_allocated_str.unwrap();
            /* If the active substring is the same length as the backing string, assume it's
             * unchanged, so we can return the backing string without reallocating. */
            if subref.len() == newly_allocated_name.as_ref().unwrap().len() {
                Cow::Owned(newly_allocated_name.unwrap())
            } else {
                let reallocated_string = subref.to_string();
                Cow::Owned(reallocated_string)
            }
        }
    }

    pub fn content_transform(&self) -> &ContentTransform {
        &self.content_transform
    }
}

trait IterateEntries {
    fn next_entry(&mut self) -> Result<Option<ZipFile>, CommandError>;
}

fn make_entry_iterator<'a>(
    err: Rc<RefCell<impl Write + 'a>>,
    input_type: InputType,
) -> Result<Box<dyn IterateEntries + 'a>, CommandError> {
    let ret: Box<dyn IterateEntries + 'a> = match input_type {
        InputType::StreamingStdin => Box::new(StdinInput::new(err)),
        InputType::ZipPaths(zip_paths) => Box::new(AllInputZips::new(err, zip_paths)?),
    };
    Ok(ret)
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
    zips_todo: VecDeque<ZipFileInput<W>>,
    cur_zip: UnsafeCell<ZipFileInput<W>>,
}

impl<W> AllInputZips<W> {
    pub fn new(
        err: Rc<RefCell<W>>,
        zip_paths: impl IntoIterator<Item = impl AsRef<Path>>,
    ) -> Result<Self, CommandError> {
        let mut zips_todo = zip_paths
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
            .collect::<Result<VecDeque<_>, CommandError>>()?;
        debug_assert!(!zips_todo.is_empty());
        let cur_zip = zips_todo.pop_front().unwrap();
        Ok(Self {
            err,
            zips_todo,
            cur_zip: UnsafeCell::new(cur_zip),
        })
    }
}

impl<W> IterateEntries for AllInputZips<W> {
    fn next_entry(&mut self) -> Result<Option<ZipFile>, CommandError> {
        loop {
            if let Some(entry) = unsafe { &mut *self.cur_zip.get() }.next_entry()? {
                return Ok(Some(entry));
            }
            match self.zips_todo.pop_front() {
                Some(zip) => {
                    self.cur_zip = UnsafeCell::new(zip);
                }
                None => {
                    return Ok(None);
                }
            }
        }
    }
}

fn process_entry_specs<W>(
    err: Rc<RefCell<W>>,
    entry_specs: impl IntoIterator<Item = EntrySpec>,
) -> Result<Vec<EntrySpecTransformer<W>>, CommandError>
where
    W: Write,
{
    let entry_spec_transformers: Vec<EntrySpecTransformer<_>> = entry_specs
        .into_iter()
        .map(|spec| EntrySpecTransformer::new(err.clone(), spec))
        .collect::<Result<_, _>>()?;
    if entry_spec_transformers.is_empty() {
        return Ok(vec![EntrySpecTransformer::empty(err.clone())]);
    };

    /* Perform some validation on the transforms since we don't currently support everything we
     * want to. */
    if entry_spec_transformers
        .iter()
        .any(|t| *t.content_transform() == ContentTransform::Raw)
    {
        /* TODO: this can be solved if we can convert a ZipFile into a Raw reader! */
        return Err(CommandError::InvalidArg(
            "--raw extraction output is not yet supported".to_string(),
        ));
    }
    if entry_spec_transformers
        .iter()
        .filter(|t| *t.content_transform() != ContentTransform::LogToStderr)
        .count()
        > 1
    {
        /* TODO: this can be solved by separating data from entries! */
        return Err(CommandError::InvalidArg(
            "more than one entry spec using a content transform which reads content (i.e. was not --log-to-stderr) was provided; this requires teeing entry contents which is not yet supported".to_string(),
        ));
    }

    Ok(entry_spec_transformers)
}

pub fn execute_extract(err: impl Write, extract: Extract) -> Result<(), CommandError> {
    let Extract {
        output,
        entry_specs,
        input,
    } = extract;
    let err = Rc::new(RefCell::new(err));

    let mut entry_receiver = make_entry_receiver(err.clone(), output)?;
    let entry_spec_transformers = process_entry_specs(err.clone(), entry_specs)?;
    let mut stderr_log_output = io::stderr();
    let mut entry_iterator = make_entry_iterator(err.clone(), input)?;

    while let Some(mut entry) = entry_iterator.next_entry()? {
        for transformer in entry_spec_transformers.iter() {
            if !transformer.matches(&entry) {
                continue;
            }
            let name = transformer.transform_name(entry.name());
            match transformer.content_transform() {
                ContentTransform::Raw => unreachable!(),
                ContentTransform::LogToStderr => {
                    writeln!(
                        &mut stderr_log_output,
                        "log to stderr: entry with original name {} and transformed name {}, compression method {}, uncompressed size {}",
                        entry.name(), name, entry.compression(), entry.size()
                    )
                    .unwrap();
                    continue;
                }
                ContentTransform::Extract => {
                    let name = name.into_owned();
                    entry_receiver.receive_entry(&mut entry, &name)?;
                }
            }
        }
    }
    entry_receiver.finalize_entries()?;

    Ok(())
}
