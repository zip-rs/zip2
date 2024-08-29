use std::{borrow::Cow, collections::VecDeque, fmt, ops, path::Path, str};

#[cfg(feature = "rx")]
use regex;

use super::matcher::{CaseSensitivity, SearchAnchoring};
use crate::{args::extract::*, CommandError};

pub trait NameTransformer: fmt::Debug {
    type Arg
    where
        Self: Sized;
    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized;
    fn transform_name<'s>(&self, name: &'s str) -> Cow<'s, str>;
}

#[derive(Debug, Copy, Clone)]
enum Trivial {
    Identity,
}

impl NameTransformer for Trivial {
    type Arg = TrivialTransform where Self: Sized;
    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        Ok(match arg {
            TrivialTransform::Identity => Self::Identity,
        })
    }
    fn transform_name<'s>(&self, name: &'s str) -> Cow<'s, str> {
        match self {
            Self::Identity => Cow::Borrowed(name),
        }
    }
}

#[derive(Debug)]
struct StripComponents {
    num_components_to_strip: usize,
}

impl NameTransformer for StripComponents {
    type Arg = u8 where Self: Sized;
    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        Ok(Self {
            num_components_to_strip: arg.into(),
        })
    }
    fn transform_name<'s>(&self, name: &'s str) -> Cow<'s, str> {
        /* If no directory components, then nothing to strip. */
        if !name.contains('/') {
            return Cow::Borrowed(name);
        }
        /* We allow stripping 0 components, which does nothing. */
        if self.num_components_to_strip == 0 {
            return Cow::Borrowed(name);
        }
        /* Pop off prefix components until only one is left or we have stripped all the
         * requested prefix components. */
        let mut remaining_to_strip = self.num_components_to_strip;
        let mut separator_indices: VecDeque<usize> =
            name.match_indices('/').map(|(i, _)| i).collect();
        debug_assert!(separator_indices.len() > 0);
        /* Always keep the final separator, as regardless of how many we strip, we want
         * to keep the basename in all cases. */
        while separator_indices.len() > 1 && remaining_to_strip > 0 {
            let _ = separator_indices.pop_front().unwrap();
            remaining_to_strip -= 1;
        }
        debug_assert!(separator_indices.len() > 0);
        let leftmost_remaining_separator_index: usize = separator_indices.pop_front().unwrap();
        Cow::Borrowed(&name[(leftmost_remaining_separator_index + 1)..])
    }
}

#[derive(Debug)]
struct AddPrefix {
    prefix_to_add: String,
}

impl NameTransformer for AddPrefix {
    type Arg = String where Self: Sized;
    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        Ok(Self { prefix_to_add: arg })
    }
    fn transform_name<'s>(&self, name: &'s str) -> Cow<'s, str> {
        /* We allow an empty prefix, which means to do nothing. */
        if self.prefix_to_add.is_empty() {
            return Cow::Borrowed(name);
        }
        Cow::Owned(format!("{}/{}", self.prefix_to_add, name))
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Multiplicity {
    #[default]
    Single,
    All,
}

impl Multiplicity {
    pub const fn from_multiple_matches_flag(multiple_matches: bool) -> Self {
        match multiple_matches {
            true => Self::All,
            false => Self::Single,
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReplaceModifiers {
    pub anchoring: SearchAnchoring,
    pub case: CaseSensitivity,
    pub multi: Multiplicity,
}

impl ReplaceModifiers {
    pub const fn from_flags(flags: PatternModifierFlags) -> Self {
        let PatternModifierFlags {
            case_insensitive,
            multiple_matches,
            prefix_anchored,
            suffix_anchored,
        } = flags;
        let multi = Multiplicity::from_multiple_matches_flag(multiple_matches);
        let case = CaseSensitivity::from_case_insensitive_flag(case_insensitive);
        let anchoring = SearchAnchoring::from_prefix_suffix_flags(prefix_anchored, suffix_anchored);
        Self {
            anchoring,
            case,
            multi,
        }
    }
}

trait PatternTransformer: fmt::Debug {
    type Replacement
    where
        Self: Sized;
    fn create(
        pattern: String,
        opts: ReplaceModifiers,
        rep: Self::Replacement,
    ) -> Result<Self, CommandError>
    where
        Self: Sized;

    fn replace<'s>(&self, input: &'s str) -> Cow<'s, str>;
}

#[derive(Debug)]
struct LiteralTransformer {
    lit: String,
    case: CaseSensitivity,
    anchoring: SearchAnchoring,
    multi: Multiplicity,
    rep: String,
}

impl LiteralTransformer {
    fn format_single_replacement<'s>(
        input: &'s str,
        lit_len: usize,
        rep: &str,
        match_index: usize,
    ) -> Cow<'s, str> {
        /* If the replacement is empty, we have the opportunity to return a borrowed Cow. */
        if rep.is_empty() {
            /* Remove the prefix alone! */
            if match_index == 0 {
                return Cow::Borrowed(&input[lit_len..]);
            }
            /* Remove the suffix alone! */
            if match_index == input.len() - lit_len {
                return Cow::Borrowed(&input[..match_index]);
            }
        }
        /* Otherwise, we allocate a new string. */
        Cow::Owned(format!(
            "{}{}{}",
            &input[..match_index],
            rep,
            &input[(match_index + lit_len)..]
        ))
    }

    fn replace_single_anchored<'s>(
        input: &'s str,
        lit: &str,
        rep: &str,
        range: ops::Range<usize>,
        case: CaseSensitivity,
    ) -> Cow<'s, str> {
        let sub = &input[range.clone()];
        if case.string_equal(lit, sub) {
            Self::format_single_replacement(input, lit.len(), rep, range.start)
        } else {
            Cow::Borrowed(input)
        }
    }

    fn replace_single_exact<'s>(input: &'s str, lit: &str, rep: &str) -> Cow<'s, str> {
        match input.find(lit) {
            None => Cow::Borrowed(input),
            Some(i) => Self::format_single_replacement(input, lit.len(), rep, i),
        }
    }

    fn replace_single_icase<'s>(input: &'s str, lit: &str, rep: &str) -> Cow<'s, str> {
        /* NB: literal was already changed to uppercase upon construction in Self::create()! */
        match input.to_ascii_uppercase().find(&lit) {
            None => Cow::Borrowed(input),
            Some(i) => Self::format_single_replacement(input, lit.len(), rep, i),
        }
    }

    fn format_multiple_replacements<'s>(
        input: &'s str,
        lit_len: usize,
        rep: &str,
        match_indices: Vec<usize>,
    ) -> Cow<'s, str> {
        if match_indices.is_empty() {
            return Cow::Borrowed(input);
        }
        if match_indices.len() == 1 {
            return Self::format_single_replacement(input, lit_len, rep, match_indices[0]);
        }
        let expected_len: usize =
            input.len() - (lit_len * match_indices.len()) + (rep.len() * match_indices.len());
        let mut ret = String::with_capacity(expected_len);
        let mut last_source_position: usize = 0;
        for i in match_indices.into_iter() {
            ret.push_str(&input[last_source_position..i]);
            ret.push_str(rep);
            last_source_position = i + lit_len;
        }
        assert_eq!(ret.len(), expected_len);
        Cow::Owned(ret)
    }

    fn replace_multiple_exact<'s>(input: &'s str, lit: &str, rep: &str) -> Cow<'s, str> {
        let match_indices: Vec<usize> = input.match_indices(lit).map(|(i, _)| i).collect();
        Self::format_multiple_replacements(input, lit.len(), rep, match_indices)
    }

    fn replace_multiple_icase<'s>(input: &'s str, lit: &str, rep: &str) -> Cow<'s, str> {
        let match_indices: Vec<usize> = input
            .to_ascii_uppercase()
            /* NB: literal was already changed to uppercase upon construction in Self::create()! */
            .match_indices(&lit)
            .map(|(i, _)| i)
            .collect();
        Self::format_multiple_replacements(input, lit.len(), rep, match_indices)
    }
}

impl PatternTransformer for LiteralTransformer {
    type Replacement = String where Self: Sized;
    fn create(
        pattern: String,
        opts: ReplaceModifiers,
        rep: Self::Replacement,
    ) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        let ReplaceModifiers {
            case,
            anchoring,
            multi,
        } = opts;

        if matches!(multi, Multiplicity::All) && !matches!(anchoring, SearchAnchoring::Unanchored) {
            return Err(CommandError::InvalidArg(format!(
                "multimatch replacement with :g is not supported with anchoring flags :p or :s for literal transforms: {opts:?} {pattern:?}"
            )));
        }

        Ok(Self {
            lit: match case {
                CaseSensitivity::Sensitive => pattern,
                CaseSensitivity::Insensitive => pattern.to_ascii_uppercase(),
            },
            case,
            anchoring,
            multi,
            rep,
        })
    }

    fn replace<'s>(&self, input: &'s str) -> Cow<'s, str> {
        /* Empty replacement or literal is allowed, it just does nothing. */
        if self.lit.is_empty() || input.is_empty() {
            return Cow::Borrowed(input);
        }
        /* Can't match input longer than the literal. */
        if self.lit.len() > input.len() {
            return Cow::Borrowed(input);
        }

        match self.multi {
            Multiplicity::Single => match self.anchoring {
                SearchAnchoring::DoublyAnchored => Self::replace_single_anchored(
                    input,
                    &self.lit,
                    &self.rep,
                    0..input.len(),
                    self.case,
                ),
                SearchAnchoring::LeftAnchored => Self::replace_single_anchored(
                    input,
                    &self.lit,
                    &self.rep,
                    0..self.lit.len(),
                    self.case,
                ),
                SearchAnchoring::RightAnchored => Self::replace_single_anchored(
                    input,
                    &self.lit,
                    &self.rep,
                    (input.len() - self.lit.len())..input.len(),
                    self.case,
                ),
                SearchAnchoring::Unanchored => match self.case {
                    CaseSensitivity::Sensitive => {
                        Self::replace_single_exact(input, &self.lit, &self.rep)
                    }
                    CaseSensitivity::Insensitive => {
                        Self::replace_single_icase(input, &self.lit, &self.rep)
                    }
                },
            },
            Multiplicity::All => match self.anchoring {
                SearchAnchoring::Unanchored => match self.case {
                    CaseSensitivity::Sensitive => {
                        Self::replace_multiple_exact(input, &self.lit, &self.rep)
                    }
                    CaseSensitivity::Insensitive => {
                        Self::replace_multiple_icase(input, &self.lit, &self.rep)
                    }
                },
                _ => unreachable!("checked during construction"),
            },
        }
    }
}

#[derive(Debug)]
#[cfg(feature = "rx")]
struct RegexpTransformer {
    pat: regex::Regex,
    multi: Multiplicity,
    rep: String,
}

#[cfg(feature = "rx")]
impl PatternTransformer for RegexpTransformer {
    type Replacement = String where Self: Sized;
    fn create(
        pattern: String,
        opts: ReplaceModifiers,
        rep: Self::Replacement,
    ) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        let ReplaceModifiers {
            case,
            anchoring,
            multi,
        } = opts;
        let pattern = anchoring.wrap_regex_pattern(&pattern);

        let pat = regex::RegexBuilder::new(&pattern)
            .case_insensitive(match case {
                CaseSensitivity::Insensitive => true,
                CaseSensitivity::Sensitive => false,
            })
            .build()
            .map_err(|e| {
                CommandError::InvalidArg(format!(
                    "failed to construct regex replacer from search pattern {pattern:?}: {e}"
                ))
            })?;
        Ok(Self { pat, multi, rep })
    }

    fn replace<'s>(&self, input: &'s str) -> Cow<'s, str> {
        match self.multi {
            Multiplicity::Single => self.pat.replace(input, &self.rep),
            Multiplicity::All => self.pat.replace_all(input, &self.rep),
        }
    }
}

pub enum ComponentSplit<'s> {
    LeftAnchored {
        selected_left: &'s str,
        right: &'s str,
    },
    RightAnchored {
        left: &'s str,
        selected_right: &'s str,
    },
    Whole(&'s str),
}

impl<'s> ComponentSplit<'s> {
    #[inline(always)]
    pub fn split_by_component_selector(sel: ComponentSelector, name: &'s str) -> Option<Self> {
        let path = Path::new(name);
        match sel {
            ComponentSelector::Path => Some(ComponentSplit::Whole(name)),
            ComponentSelector::Basename => path
                .file_name()
                .map(|bname| bname.to_str().unwrap())
                .map(|bname| name.split_at(name.len() - bname.len()))
                .map(|(pfx, bname)| ComponentSplit::RightAnchored {
                    left: pfx,
                    selected_right: bname,
                }),
            ComponentSelector::Dirname => path
                .parent()
                .map(|p| p.to_str().unwrap())
                /* "a".parent() becomes Some(""), which we want to treat as no parent */
                .filter(|s| !s.is_empty())
                .map(|dirname| name.split_at(dirname.len()))
                .map(|(dirname, sfx)| ComponentSplit::LeftAnchored {
                    selected_left: dirname,
                    right: sfx,
                }),
            ComponentSelector::FileExtension => path
                .extension()
                .map(|ext| ext.to_str().unwrap())
                .map(|ext| name.split_at(name.len() - ext.len()))
                .map(|(pfx, ext)| ComponentSplit::RightAnchored {
                    left: pfx,
                    selected_right: ext,
                }),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum SubstringAnchoring {
    RetainsLeftAnchor,
    RetainsRightAnchor,
    RetainsBothAnchors,
    LosesBothAnchors,
}

impl SubstringAnchoring {
    #[inline(always)]
    pub fn analyze<'s, 't>(parent: &'s str, sub: &'t str) -> Self
    where
        't: 's,
    {
        let p = parent.as_bytes().as_ptr_range();
        let s = sub.as_bytes().as_ptr_range();
        assert!(s.start >= p.start);
        assert!(s.end <= p.end);
        if p.start == s.start {
            if p.end == s.end {
                debug_assert_eq!(parent, sub);
                Self::RetainsBothAnchors
            } else {
                Self::RetainsLeftAnchor
            }
        } else {
            if p.end == s.end {
                Self::RetainsRightAnchor
            } else {
                Self::LosesBothAnchors
            }
        }
    }

    #[inline(always)]
    pub fn split_then_transform_then_reformulate<'s>(
        input: &'s str,
        split: impl FnOnce(&'s str) -> Option<ComponentSplit<'s>>,
        transform: impl FnOnce(&'s str) -> Cow<'s, str>,
    ) -> Cow<'s, str> {
        let components = match split(input) {
            /* If the given name doesn't have the specified component, return it unchanged. */
            None => return Cow::Borrowed(input),
            Some(s) => s,
        };
        match components {
            /* If there was no splitting (the whole path was selected), then we don't need to do
             * any work to hook things back up! */
            ComponentSplit::Whole(s) => transform(s),
            /* If there was splitting, we need to do more work. */
            ComponentSplit::LeftAnchored {
                selected_left,
                right,
            } => match transform(selected_left) {
                /* If we reallocated, then we have to reallocate the whole thing, so reuse the
                 * returned String. */
                Cow::Owned(mut new_left) => {
                    new_left.push_str(right);
                    Cow::Owned(new_left)
                }
                /* If no reallocation, we now have to figure out whether the result is still
                 * contiguous. */
                Cow::Borrowed(left_sub) => match Self::analyze(selected_left, left_sub) {
                    Self::RetainsBothAnchors => Cow::Borrowed(input),
                    Self::RetainsRightAnchor => {
                        Cow::Borrowed(Self::join_adjacent_strings(input, left_sub, right))
                    }
                    _ => Cow::Owned(format!("{}{}", left_sub, right)),
                },
            },
            ComponentSplit::RightAnchored {
                left,
                selected_right,
            } => match transform(selected_right) {
                Cow::Owned(mut new_right) => {
                    new_right.insert_str(0, left);
                    Cow::Owned(new_right)
                }
                Cow::Borrowed(right_sub) => match Self::analyze(selected_right, right_sub) {
                    Self::RetainsBothAnchors => Cow::Borrowed(input),
                    Self::RetainsLeftAnchor => {
                        Cow::Borrowed(Self::join_adjacent_strings(input, left, right_sub))
                    }
                    _ => Cow::Owned(format!("{}{}", left, right_sub)),
                },
            },
        }
    }

    #[inline(always)]
    fn join_adjacent_strings<'s, 't>(parent: &'s str, left: &'t str, right: &'t str) -> &'s str
    where
        't: 's,
    {
        let parent_range = parent.as_bytes().as_ptr_range();
        let left = left.as_bytes().as_ptr_range();
        debug_assert!(left.start >= parent_range.start && left.end <= parent_range.end);
        let right = right.as_bytes().as_ptr_range();
        debug_assert!(right.start >= parent_range.start && right.end <= parent_range.end);
        debug_assert_eq!(left.end, right.start);
        let start_offset = (left.start as usize) - (parent_range.start as usize);
        let end_offset = (parent_range.end as usize) - (right.end as usize);
        &parent[start_offset..(parent.len() - end_offset)]
    }
}

#[derive(Debug)]
struct ComponentTransformer {
    pattern_trans: Box<dyn PatternTransformer>,
    comp_sel: ComponentSelector,
}

impl NameTransformer for ComponentTransformer {
    type Arg = TransformArg where Self: Sized;
    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        let TransformArg {
            comp_sel,
            pat_sel: PatternSelector { pat_sel, modifiers },
            pattern,
            replacement_spec,
        } = arg;

        let opts = ReplaceModifiers::from_flags(modifiers);
        let pattern_trans: Box<dyn PatternTransformer> = match pat_sel {
            PatternSelectorType::Glob => {
                return Err(CommandError::InvalidArg(format!(
                    "glob patterns are not supported for name transformations: {pattern:?}"
                )));
            }
            PatternSelectorType::Literal => {
                Box::new(LiteralTransformer::create(pattern, opts, replacement_spec)?)
            }
            PatternSelectorType::Regexp => {
                #[cfg(feature = "rx")]
                {
                    Box::new(RegexpTransformer::create(pattern, opts, replacement_spec)?)
                }
                #[cfg(not(feature = "rx"))]
                {
                    return Err(CommandError::InvalidArg(format!(
                        "regexp patterns were requested, but this binary was built without the \"rx\" feature: {pattern:?}"
                    )));
                }
            }
        };

        Ok(Self {
            pattern_trans,
            comp_sel,
        })
    }

    fn transform_name<'s>(&self, name: &'s str) -> Cow<'s, str> {
        SubstringAnchoring::split_then_transform_then_reformulate(
            name,
            move |name| ComponentSplit::split_by_component_selector(self.comp_sel, name),
            |name| self.pattern_trans.replace(name),
        )
    }
}

#[derive(Debug)]
pub struct CompiledTransformer {
    transformers: Vec<Box<dyn NameTransformer>>,
}

impl CompiledTransformer {
    fn make_single(trans: NameTransform) -> Result<Box<dyn NameTransformer>, CommandError> {
        Ok(match trans {
            NameTransform::Trivial(arg) => Box::new(Trivial::from_arg(arg)?),
            NameTransform::Basic(basic_trans) => match basic_trans {
                BasicTransform::StripComponents(arg) => Box::new(StripComponents::from_arg(arg)?),
                BasicTransform::AddPrefix(arg) => Box::new(AddPrefix::from_arg(arg)?),
            },
            NameTransform::Complex(complex_trans) => match complex_trans {
                ComplexTransform::Transform(arg) => Box::new(ComponentTransformer::from_arg(arg)?),
            },
        })
    }
}

impl NameTransformer for CompiledTransformer {
    type Arg = Vec<NameTransform> where Self: Sized;
    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        assert!(!arg.is_empty());
        Ok(Self {
            transformers: arg
                .into_iter()
                .map(Self::make_single)
                .collect::<Result<_, _>>()?,
        })
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
    fn transform_name<'s>(&self, mut original_name: &'s str) -> Cow<'s, str> {
        let mut newly_allocated_name: Option<String> = None;
        let mut newly_allocated_str: Option<&str> = None;
        for transformer in self.transformers.iter() {
            match newly_allocated_str {
                Some(s) => match transformer.transform_name(s) {
                    Cow::Borrowed(t) => {
                        let _ = newly_allocated_str.replace(t);
                    }
                    Cow::Owned(t) => {
                        assert!(newly_allocated_name.replace(t).is_some());
                        newly_allocated_str = Some(newly_allocated_name.as_ref().unwrap().as_str());
                    }
                },
                None => match transformer.transform_name(original_name) {
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
}
