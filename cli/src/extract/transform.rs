use std::{borrow::Cow, collections::VecDeque};

use zip::read::ZipFile;

use crate::{args::extract::*, CommandError};

use super::matcher::{EntryMatcher, WrappedMatcher};

struct Transformer {
    trans: NameTransform,
}

impl Transformer {
    pub fn new(trans: NameTransform) -> Self {
        Self { trans }
    }
}

impl Transformer {
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

pub struct EntrySpecTransformer {
    matcher: Option<WrappedMatcher>,
    name_transformers: Vec<Transformer>,
    content_transform: ContentTransform,
}

impl EntrySpecTransformer {
    pub fn new(entry_spec: EntrySpec) -> Result<Self, CommandError> {
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
            .map(|trans| Transformer::new(trans))
            .collect();
        Ok(Self {
            matcher,
            name_transformers,
            content_transform,
        })
    }

    pub fn empty() -> Self {
        Self {
            matcher: None,
            name_transformers: Vec::new(),
            content_transform: ContentTransform::Extract,
        }
    }
}

impl EntrySpecTransformer {
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

pub fn process_entry_specs(
    entry_specs: impl IntoIterator<Item = EntrySpec>,
) -> Result<Vec<EntrySpecTransformer>, CommandError> {
    let entry_spec_transformers: Vec<EntrySpecTransformer> = entry_specs
        .into_iter()
        .map(|spec| EntrySpecTransformer::new(spec))
        .collect::<Result<_, _>>()?;
    if entry_spec_transformers.is_empty() {
        return Ok(vec![EntrySpecTransformer::empty()]);
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
