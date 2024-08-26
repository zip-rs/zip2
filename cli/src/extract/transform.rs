use std::{borrow::Cow, collections::VecDeque};

use regex;

use crate::{args::extract::*, CommandError};

pub trait NameTransformer {
    type Arg
    where
        Self: Sized;
    fn from_arg(arg: Self::Arg) -> Result<Self, CommandError>
    where
        Self: Sized;
    fn transform_name<'s>(&self, name: &'s str) -> Cow<'s, str>;
}

#[derive(Copy, Clone)]
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

trait PatternTransformer {
    type Replacement
    where
        Self: Sized;
    fn create(
        pattern: &str,
        opts: PatternModifiers,
        rep: Self::Replacement,
    ) -> Result<Self, CommandError>
    where
        Self: Sized;

    fn replace<'s>(&self, input: &'s str) -> Cow<'s, str>;
}

struct LiteralTransformer {
    lit: String,
    case_insensitive: bool,
    multiple_matches: bool,
    rep: String,
}

impl LiteralTransformer {
    fn format_single_replacement(
        input: &str,
        lit_len: usize,
        rep: &str,
        match_index: usize,
    ) -> String {
        debug_assert!(lit_len > 0);
        debug_assert!(input.len() > 0);
        debug_assert!(rep.len() > 0);
        format!(
            "{}{}{}",
            &input[..match_index],
            rep,
            &input[(match_index + lit_len)..]
        )
    }

    fn replace_single_exact<'s>(input: &'s str, lit: &str, rep: &str) -> Cow<'s, str> {
        match input.find(lit) {
            None => Cow::Borrowed(input),
            Some(i) => Cow::Owned(Self::format_single_replacement(input, lit.len(), rep, i)),
        }
    }

    fn replace_single_icase<'s>(input: &'s str, lit: &str, rep: &str) -> Cow<'s, str> {
        /* NB: literal was already changed to uppercase upon construction in Self::create()! */
        match input.to_ascii_uppercase().find(&lit) {
            None => Cow::Borrowed(input),
            Some(i) => Cow::Owned(Self::format_single_replacement(input, lit.len(), rep, i)),
        }
    }

    fn format_multiple_replacements(
        input: &str,
        lit_len: usize,
        rep: &str,
        match_indices: Vec<usize>,
    ) -> String {
        debug_assert!(lit_len > 0);
        debug_assert!(input.len() > 0);
        debug_assert!(rep.len() > 0);
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
        ret
    }

    fn replace_multiple_exact<'s>(input: &'s str, lit: &str, rep: &str) -> Cow<'s, str> {
        let match_indices: Vec<usize> = input.match_indices(lit).map(|(i, _)| i).collect();
        if match_indices.is_empty() {
            return Cow::Borrowed(input);
        }
        Cow::Owned(Self::format_multiple_replacements(
            input,
            lit.len(),
            rep,
            match_indices,
        ))
    }

    fn replace_multiple_icase<'s>(input: &'s str, lit: &str, rep: &str) -> Cow<'s, str> {
        let match_indices: Vec<usize> = input
            .to_ascii_uppercase()
            /* NB: literal was already changed to uppercase upon construction in Self::create()! */
            .match_indices(&lit)
            .map(|(i, _)| i)
            .collect();
        if match_indices.is_empty() {
            return Cow::Borrowed(input);
        }
        Cow::Owned(Self::format_multiple_replacements(
            input,
            lit.len(),
            rep,
            match_indices,
        ))
    }
}

impl PatternTransformer for LiteralTransformer {
    type Replacement = String where Self: Sized;
    fn create(
        pattern: &str,
        opts: PatternModifiers,
        rep: Self::Replacement,
    ) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        let PatternModifiers {
            case_insensitive,
            multiple_matches,
        } = opts;
        Ok(Self {
            lit: match case_insensitive {
                false => pattern.to_string(),
                true => pattern.to_ascii_uppercase(),
            },
            case_insensitive,
            multiple_matches,
            rep,
        })
    }

    fn replace<'s>(&self, input: &'s str) -> Cow<'s, str> {
        /* Empty replacement or literal is allowed, it just does nothing. */
        if self.lit.is_empty() || self.rep.is_empty() || input.is_empty() {
            return Cow::Borrowed(input);
        }
        match self.multiple_matches {
            false => match self.case_insensitive {
                /* Single replacement, case-sensitive (exact) match: */
                false => Self::replace_single_exact(input, &self.lit, &self.rep),
                /* Single replacement, case-insensitive match: */
                true => Self::replace_single_icase(input, &self.lit, &self.rep),
            },
            true => match self.case_insensitive {
                /* Multiple replacements, case-sensitive (exact) match: */
                false => Self::replace_multiple_exact(input, &self.lit, &self.rep),
                /* Multiple replacements, case-insensitive match: */
                true => Self::replace_multiple_icase(input, &self.lit, &self.rep),
            },
        }
    }
}

struct RegexpTransformer {
    pat: regex::Regex,
    multiple_matches: bool,
    rep: String,
}

impl PatternTransformer for RegexpTransformer {
    type Replacement = String where Self: Sized;
    fn create(
        pattern: &str,
        opts: PatternModifiers,
        rep: Self::Replacement,
    ) -> Result<Self, CommandError>
    where
        Self: Sized,
    {
        let PatternModifiers {
            case_insensitive,
            multiple_matches,
        } = opts;
        let pat = regex::RegexBuilder::new(pattern)
            .case_insensitive(case_insensitive)
            .build()
            .map_err(|e| {
                CommandError::InvalidArg(format!(
                    "failed to construct regex replacer from search pattern {pattern:?}: {e}"
                ))
            })?;
        Ok(Self {
            pat,
            multiple_matches,
            rep,
        })
    }

    fn replace<'s>(&self, input: &'s str) -> Cow<'s, str> {
        match self.multiple_matches {
            false => self.pat.replace(input, &self.rep),
            true => self.pat.replace_all(input, &self.rep),
        }
    }
}

/* struct ComponentTransformer { */
/*     pattern_trans: Box<dyn PatternTransformer>, */
/*     comp_sel: ComponentSelector, */
/* } */

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
                ComplexTransform::Transform(transform_arg) => {
                    todo!("impl transform: {:?}", transform_arg)
                }
                ComplexTransform::RemovePrefix(remove_prefix_arg) => {
                    todo!("impl remove prefix: {:?}", remove_prefix_arg)
                }
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
