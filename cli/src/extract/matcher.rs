use std::{borrow::Cow, fmt};

#[cfg(feature = "glob")]
use glob;
#[cfg(feature = "rx")]
use regex;

use zip::CompressionMethod;

use super::receiver::{EntryData, EntryKind};
use super::transform::ComponentSplit;
use crate::{args::extract::*, CommandError};

#[inline(always)]
fn process_component_selector<'s>(sel: ComponentSelector, name: &'s str) -> Option<&'s str> {
    ComponentSplit::split_by_component_selector(sel, name).map(|split| match split {
        ComponentSplit::LeftAnchored { selected_left, .. } => selected_left,
        ComponentSplit::RightAnchored { selected_right, .. } => selected_right,
        ComponentSplit::Whole(s) => s,
    })
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SearchAnchoring {
    #[default]
    Unanchored,
    LeftAnchored,
    RightAnchored,
    DoublyAnchored,
}

impl SearchAnchoring {
    pub const fn from_prefix_suffix_flags(prefix_anchored: bool, suffix_anchored: bool) -> Self {
        match (prefix_anchored, suffix_anchored) {
            (true, true) => Self::DoublyAnchored,
            (true, false) => Self::LeftAnchored,
            (false, true) => Self::RightAnchored,
            (false, false) => Self::Unanchored,
        }
    }

    pub fn wrap_regex_pattern<'s>(self, pattern: &'s str) -> Cow<'s, str> {
        match self {
            Self::Unanchored => Cow::Borrowed(pattern),
            Self::LeftAnchored => Cow::Owned(format!("^(?:{pattern})")),
            Self::RightAnchored => Cow::Owned(format!("(?:{pattern})$")),
            Self::DoublyAnchored => Cow::Owned(format!("^(?:{pattern})$")),
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CaseSensitivity {
    #[default]
    Sensitive,
    Insensitive,
}

impl CaseSensitivity {
    pub const fn from_case_insensitive_flag(case_insensitive: bool) -> Self {
        match case_insensitive {
            true => Self::Insensitive,
            false => Self::Sensitive,
        }
    }

    pub fn string_equal(self, a: &str, b: &str) -> bool {
        match self {
            Self::Insensitive => a.eq_ignore_ascii_case(b),
            Self::Sensitive => a == b,
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MatchModifiers {
    pub anchoring: SearchAnchoring,
    pub case: CaseSensitivity,
}

impl MatchModifiers {
    pub fn from_flags(flags: PatternModifierFlags) -> Result<Self, CommandError> {
        let PatternModifierFlags {
            case_insensitive,
            multiple_matches,
            prefix_anchored,
            suffix_anchored,
        } = flags;
        if multiple_matches {
            return Err(CommandError::InvalidArg(format!(
                "multimatch modifier :g is unused in match expressions: {flags:?}"
            )));
        }
        let case = CaseSensitivity::from_case_insensitive_flag(case_insensitive);
        let anchoring = SearchAnchoring::from_prefix_suffix_flags(prefix_anchored, suffix_anchored);
        Ok(Self { anchoring, case })
    }
}

trait NameMatcher: fmt::Debug {
    fn create(pattern: String, opts: MatchModifiers) -> Result<Self, CommandError>
    where
        Self: Sized;
    fn matches(&self, input: &str) -> bool;
}

#[derive(Debug)]
struct LiteralMatcher {
    lit: String,
    case: CaseSensitivity,
    anchoring: SearchAnchoring,
}

impl NameMatcher for LiteralMatcher {
    fn create(pattern: String, opts: MatchModifiers) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        let MatchModifiers { case, anchoring } = opts;
        Ok(Self {
            lit: match case {
                CaseSensitivity::Sensitive => pattern,
                CaseSensitivity::Insensitive => pattern.to_ascii_uppercase(),
            },
            case,
            anchoring,
        })
    }

    fn matches(&self, input: &str) -> bool {
        if input.len() < self.lit.len() {
            return false;
        }
        match self.anchoring {
            SearchAnchoring::Unanchored => match self.case {
                CaseSensitivity::Insensitive => input.to_ascii_uppercase().contains(&self.lit),
                CaseSensitivity::Sensitive => input.contains(&self.lit),
            },
            SearchAnchoring::DoublyAnchored => self.case.string_equal(&self.lit, input),
            SearchAnchoring::LeftAnchored => {
                let prefix = &input[..self.lit.len()];
                self.case.string_equal(&self.lit, prefix)
            }
            SearchAnchoring::RightAnchored => {
                let suffix = &input[(input.len() - self.lit.len())..];
                self.case.string_equal(&self.lit, suffix)
            }
        }
    }
}

#[derive(Debug)]
#[cfg(feature = "glob")]
struct GlobMatcher {
    pat: glob::Pattern,
    glob_opts: glob::MatchOptions,
}

#[cfg(feature = "glob")]
impl NameMatcher for GlobMatcher {
    fn create(pattern: String, opts: MatchModifiers) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        let MatchModifiers { anchoring, case } = opts;
        if !matches!(anchoring, SearchAnchoring::Unanchored) {
            return Err(CommandError::InvalidArg(format!(
                "anchored search with :p or :s is incompatible with glob patterns: {opts:?}"
            )));
        }
        let glob_opts = glob::MatchOptions {
            case_sensitive: match case {
                CaseSensitivity::Sensitive => true,
                CaseSensitivity::Insensitive => false,
            },
            ..Default::default()
        };
        let pat = glob::Pattern::new(&pattern).map_err(|e| {
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

#[derive(Debug)]
#[cfg(feature = "rx")]
struct RegexMatcher {
    pat: regex::Regex,
}

#[cfg(feature = "rx")]
impl NameMatcher for RegexMatcher {
    fn create(pattern: String, opts: MatchModifiers) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        let MatchModifiers { case, anchoring } = opts;

        let pattern = anchoring.wrap_regex_pattern(&pattern);

        let pat = regex::RegexBuilder::new(&pattern)
            .case_insensitive(match case {
                CaseSensitivity::Sensitive => false,
                CaseSensitivity::Insensitive => true,
            })
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

pub trait EntryMatcher: fmt::Debug {
    type Arg
    where
        Self: Sized;
    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized;
    fn matches(&self, entry: &EntryData) -> bool;
}

#[derive(Debug, Copy, Clone)]
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

    fn matches(&self, _entry: &EntryData) -> bool {
        match self {
            Self::True => true,
            Self::False => false,
        }
    }
}

#[derive(Debug, Copy, Clone)]
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

    fn matches(&self, entry: &EntryData) -> bool {
        match (self, entry.kind) {
            (Self::File, EntryKind::File) => true,
            (Self::Dir, EntryKind::Dir) => true,
            (Self::Symlink, EntryKind::Symlink) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Copy, Clone)]
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

    fn matches(&self, entry: &EntryData) -> bool {
        match self {
            Self::Any => true,
            Self::Known => {
                SpecificCompressionMethodArg::KNOWN_COMPRESSION_METHODS.contains(&entry.compression)
            }
        }
    }
}

#[derive(Debug)]
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

    fn matches(&self, entry: &EntryData) -> bool {
        self.specific_method == entry.compression
    }
}

#[derive(Debug, Copy, Clone)]
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

    fn matches(&self, entry: &EntryData) -> bool {
        let num_components = entry.name.split('/').count();
        match self {
            Self::Max(max) => num_components <= *max,
            Self::Min(min) => num_components >= *min,
        }
    }
}

#[derive(Debug, Copy, Clone)]
enum Size {
    Max(u64),
    Min(u64),
}

impl EntryMatcher for Size {
    type Arg = SizeArg where Self: Sized;

    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        Ok(match arg {
            SizeArg::Max(max) => Self::Max(max),
            SizeArg::Min(min) => Self::Min(min),
        })
    }

    fn matches(&self, entry: &EntryData) -> bool {
        match self {
            Self::Max(max) => entry.uncompressed_size <= *max,
            Self::Min(min) => entry.uncompressed_size >= *min,
        }
    }
}

#[derive(Debug)]
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

        let opts = MatchModifiers::from_flags(modifiers)?;
        let matcher: Box<dyn NameMatcher> = match pat_sel {
            PatternSelectorType::Glob => {
                #[cfg(feature = "glob")]
                {
                    Box::new(GlobMatcher::create(pattern, opts)?)
                }
                #[cfg(not(feature = "glob"))]
                {
                    return Err(CommandError::InvalidArg(format!(
                        "glob patterns were requested, but this binary was built without the \"glob\" feature: {pattern:?}"
                    )));
                }
            }

            PatternSelectorType::Literal => Box::new(LiteralMatcher::create(pattern, opts)?),
            PatternSelectorType::Regexp => {
                #[cfg(feature = "rx")]
                {
                    Box::new(RegexMatcher::create(pattern, opts)?)
                }
                #[cfg(not(feature = "rx"))]
                {
                    return Err(CommandError::InvalidArg(format!(
                        "regexp patterns were requested, but this binary was built without the \"rx\" feature: {pattern:?}"
                    )));
                }
            }
        };

        Ok(Self { matcher, comp_sel })
    }

    fn matches(&self, entry: &EntryData) -> bool {
        match process_component_selector(self.comp_sel, entry.name) {
            None => false,
            Some(s) => self.matcher.matches(s),
        }
    }
}

#[derive(Debug)]
pub enum CompiledMatcher {
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

impl CompiledMatcher {
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
            Predicate::Size(arg) => Box::new(Size::from_arg(arg)?),
            Predicate::Match(arg) => Box::new(PatternMatcher::from_arg(arg)?),
        }))
    }
}

impl EntryMatcher for CompiledMatcher {
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

    fn matches(&self, entry: &EntryData) -> bool {
        match self {
            Self::Primitive(m) => m.matches(entry),
            Self::Negated(m) => !m.matches(entry),
            Self::And { left, right } => left.matches(entry) && right.matches(entry),
            Self::Or { left, right } => left.matches(entry) || right.matches(entry),
        }
    }
}
