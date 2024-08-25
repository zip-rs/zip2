use std::{
    cell::RefCell,
    io::{self, Write},
    rc::Rc,
};

use crate::{args::extract::*, CommandError};

mod entries;
mod matcher;
mod receiver;
mod transform;

pub fn execute_extract(err: impl Write, extract: Extract) -> Result<(), CommandError> {
    let Extract {
        output,
        entry_specs,
        input,
    } = extract;
    let err = Rc::new(RefCell::new(err));

    let mut entry_receiver = receiver::make_entry_receiver(err.clone(), output)?;
    let entry_spec_transformers = transform::process_entry_specs(entry_specs)?;
    let mut stderr_log_output = io::stderr();
    let mut entry_iterator = entries::make_entry_iterator(input)?;

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
