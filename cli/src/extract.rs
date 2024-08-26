use std::{
    borrow::Cow,
    cell::RefCell,
    io::{self, Read, Write},
    rc::Rc,
};

use crate::{args::extract::*, CommandError, WrapCommandErr};

mod entries;
mod matcher;
mod receiver;
mod transform;
use entries::IterateEntries;
use matcher::EntryMatcher;
use receiver::{CompiledEntrySpec, ConcatEntry, EntryData, EntryReceiver, ExtractEntry};
use transform::NameTransformer;

pub fn execute_extract(mut err: impl Write, extract: Extract) -> Result<(), CommandError> {
    let Extract {
        output_specs,
        entry_specs,
        input_spec,
    } = extract;

    let compiled_specs = receiver::process_entry_and_output_specs(entry_specs, output_specs)?;
    let mut entry_iterator = entries::MergedInput::from_spec(input_spec)?;

    let mut copy_buf: Vec<u8> = vec![0u8; 1024 * 16];

    while let Some(mut entry) = entry_iterator.next_entry()? {
        let data = EntryData::from_entry(&entry);

        let mut matching_concats: Vec<Rc<RefCell<dyn Write>>> = Vec::new();
        let mut matching_extracts: Vec<(Cow<'_, str>, Rc<dyn EntryReceiver>)> = Vec::new();
        for spec in compiled_specs.iter() {
            match spec {
                CompiledEntrySpec::Concat(ConcatEntry { matcher, stream }) => {
                    if matcher.as_ref().map(|m| m.matches(&data)).unwrap_or(true) {
                        matching_concats.push(stream.clone());
                    }
                }
                CompiledEntrySpec::Extract(ExtractEntry {
                    matcher,
                    transforms,
                    recv,
                }) => {
                    if matcher.as_ref().map(|m| m.matches(&data)).unwrap_or(true) {
                        let new_name = transforms
                            .as_ref()
                            .map(|t| t.transform_name(&data.name))
                            .unwrap_or_else(|| Cow::Borrowed(&data.name));
                        matching_extracts.push((new_name, recv.clone()));
                    }
                }
            }
        }
        if matching_concats.is_empty() && matching_extracts.is_empty() {
            continue;
        }

        /* Split output handles for concat, and split generated handles by extract source and
         * name. use Rc::ptr_eq() to split, and Cow::<'s, str>::eq() with str AsRef. */
        let mut deduped_concat_writers: Vec<Rc<RefCell<dyn Write>>> = Vec::new();
        for concat_p in matching_concats.into_iter() {
            if deduped_concat_writers
                .iter()
                .any(|p| Rc::ptr_eq(p, &concat_p))
            {
                writeln!(&mut err, "skipping repeated concat").unwrap();
            } else {
                deduped_concat_writers.push(concat_p);
            }
        }
        let mut deduped_matching_extracts: Vec<(Cow<'_, str>, Rc<dyn EntryReceiver>)> = Vec::new();
        for (name, extract_p) in matching_extracts.into_iter() {
            if deduped_matching_extracts
                .iter()
                .any(|(n, p)| Rc::ptr_eq(p, &extract_p) && name.as_ref() == n.as_ref())
            {
                writeln!(&mut err, "skipping repeated extract").unwrap();
            } else {
                deduped_matching_extracts.push((name, extract_p));
            }
        }

        let mut matching_handles: Vec<Box<dyn Write>> = deduped_matching_extracts
            .into_iter()
            .map(|(name, recv)| recv.generate_entry_handle(name))
            .collect::<Result<_, _>>()?;

        let mut read_len: usize = 0;
        loop {
            read_len = entry.read(&mut copy_buf).wrap_err("read of entry failed")?;
            if read_len == 0 {
                break;
            }
            let cur_data: &[u8] = &copy_buf[..read_len];
            for concat_writer in deduped_concat_writers.iter() {
                concat_writer
                    .borrow_mut()
                    .write_all(cur_data)
                    .wrap_err("failed to write data to concat output")?;
            }
            for extract_writer in matching_handles.iter_mut() {
                extract_writer
                    .write_all(cur_data)
                    .wrap_err("failed to write data to extract output")?;
            }
        }
    }
    /* Finalize all extract entries. */
    for spec in compiled_specs.into_iter() {
        match spec {
            CompiledEntrySpec::Concat(_) => (),
            CompiledEntrySpec::Extract(ExtractEntry { recv, .. }) => {
                recv.finalize_entries()?;
            }
        }
    }

    Ok(())
}
