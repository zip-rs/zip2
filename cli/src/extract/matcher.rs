use glob;
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
        let PatternModifiers {
            case_insensitive, ..
        } = opts;
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
        let PatternModifiers {
            case_insensitive, ..
        } = opts;
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
        let PatternModifiers {
            case_insensitive, ..
        } = opts;
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

pub trait EntryMatcher {
    type Arg
    where
        Self: Sized;
    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized;
    fn matches(&self, entry: &EntryData) -> bool;
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

    fn matches(&self, _entry: &EntryData) -> bool {
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

    fn matches(&self, entry: &EntryData) -> bool {
        match (self, entry.kind) {
            (Self::File, EntryKind::File) => true,
            (Self::Dir, EntryKind::Dir) => true,
            (Self::Symlink, EntryKind::Symlink) => true,
            _ => false,
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

    fn matches(&self, entry: &EntryData) -> bool {
        match self {
            Self::Any => true,
            Self::Known => {
                SpecificCompressionMethodArg::KNOWN_COMPRESSION_METHODS.contains(&entry.compression)
            }
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

    fn matches(&self, entry: &EntryData) -> bool {
        self.specific_method == entry.compression
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

    fn matches(&self, entry: &EntryData) -> bool {
        let num_components = entry.name.split('/').count();
        match self {
            Self::Max(max) => num_components <= *max,
            Self::Min(min) => num_components >= *min,
        }
    }
}

#[derive(Copy, Clone)]
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
            Self::Max(max) => entry.size <= *max,
            Self::Min(min) => entry.size >= *min,
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

    fn matches(&self, entry: &EntryData) -> bool {
        match process_component_selector(self.comp_sel, entry.name) {
            None => false,
            Some(s) => self.matcher.matches(s),
        }
    }
}

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
