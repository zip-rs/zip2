use std::{
    borrow::Cow,
    cell::RefCell,
    fs,
    io::{self, Read, Write},
    rc::Rc,
};

use zip::read::{ZipArchive, ZipFile};

use crate::{args::extract::*, CommandError, WrapCommandErr};

pub mod entries;
pub mod matcher;
pub mod named_outputs;
pub mod receiver;
pub mod transform;
use entries::{IterateEntries, StreamInput, ZipFileInput};
use receiver::{CompiledEntrySpec, EntryData, EntryKind, EntryReceiver, ExtractEntry};

fn maybe_process_symlink<'a, 't>(
    entry: &mut ZipFile<'a>,
    err: &Rc<RefCell<impl Write>>,
    symlink_target: &'t mut Vec<u8>,
) -> Result<Option<&'t mut [u8]>, CommandError> {
    let (kind, size) = {
        /* FIXME: the ZipFile<'a> struct contains a *mutable* reference to the parent archive,
         *        and this actually imposes a mutable reference upon any references to the
         *        immutable ZipFileData contents. This means we cannot have any immutable
         *        references to the ZipFileData contents at the same time as a mutable
         *        reference. What this means here is that we have to create a temporary EntryData
         *        struct and then immediately throw it away in order to be able to read the entry
         *        contents with io::Read. ZipEntry<'a, R> from
         *        https://github.com/zip-rs/zip2/pull/233 avoids this issue!!! */
        let data = EntryData::from_entry(&entry);
        (data.kind, data.uncompressed_size)
    };
    if !matches!(kind, EntryKind::Symlink) {
        return Ok(None);
    }

    /* We can't read the entry name from EntryData because we can't have any immutable
     * references to ZipFileData like the name at the same time we use the entry as
     * a reader! That means our log message here is very unclear! */
    writeln!(&mut err.borrow_mut(), "reading symlink target").unwrap();
    /* Re-use the vector allocation, but make sure to avoid re-using the symlink data from
     * a previous iteration. */
    symlink_target.clear();
    entry
        .read_to_end(symlink_target)
        .wrap_err("failed to read symlink target from zip archive entry")?;
    debug_assert_eq!(symlink_target.len(), size.try_into().unwrap());
    Ok(Some(symlink_target))
}

fn process_entry<'a, 'w, 'c, 'it>(
    mut entry: ZipFile<'a>,
    err: &Rc<RefCell<impl Write>>,
    compiled_specs: impl Iterator<Item = &'it CompiledEntrySpec<'w>>,
    copy_buf: &mut [u8],
    symlink_target: &mut Vec<u8>,
    deduped_concat_writers: &mut Vec<&'c Rc<RefCell<dyn Write + 'w>>>,
    matching_handles: &mut Vec<Box<dyn Write + '_>>,
) -> Result<(), CommandError>
where
    'w: 'it,
    'it: 'c,
{
    deduped_concat_writers.clear();
    matching_handles.clear();

    let symlink_target = maybe_process_symlink(&mut entry, err, symlink_target)?;
    /* We dropped any mutable handles to the entry, so now we can access its metadata again. */
    let data = EntryData::from_entry(&entry);

    let mut deduped_matching_extracts: Vec<(&'c Rc<dyn EntryReceiver + 'w>, Vec<Cow<'_, str>>)> =
        Vec::new();
    for matching_spec in compiled_specs.filter_map(|spec| spec.try_match_and_transform(&data)) {
        if matching_spec.is_nested_duplicate(deduped_concat_writers, &mut deduped_matching_extracts)
        {
            writeln!(&mut err.borrow_mut(), "skipping repeated output").unwrap();
        }
    }

    matching_handles.extend(
        deduped_matching_extracts
            .into_iter()
            .flat_map(|(recv, names)| names.into_iter().map(move |n| (recv, n)))
            .map(|(recv, name)| recv.generate_entry_handle(&data, symlink_target.as_deref(), name))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten(),
    );

    let mut read_len: usize;
    loop {
        read_len = entry.read(copy_buf).wrap_err("read of entry failed")?;
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

    Ok(())
}

pub fn execute_extract(err: impl Write, extract: Extract) -> Result<(), CommandError> {
    let Extract {
        output_specs,
        entry_specs,
        input_spec: InputSpec {
            stdin_stream,
            zip_paths,
        },
    } = extract;
    let err = Rc::new(RefCell::new(err));

    writeln!(&mut err.borrow_mut(), "entry specs: {entry_specs:?}").unwrap();
    let compiled_specs =
        named_outputs::process_entry_and_output_specs(err.clone(), entry_specs, output_specs)?;
    writeln!(&mut err.borrow_mut(), "compiled specs: {compiled_specs:?}").unwrap();

    let mut copy_buf: Vec<u8> = vec![0u8; 1024 * 16];
    let mut symlink_target: Vec<u8> = Vec::new();

    let mut deduped_concat_writers: Vec<&Rc<RefCell<dyn Write + '_>>> = Vec::new();
    let mut matching_handles: Vec<Box<dyn Write + '_>> = Vec::new();

    if stdin_stream {
        writeln!(&mut err.borrow_mut(), "extracting from stdin").unwrap();
        let mut stdin = StreamInput::new(io::stdin().lock());

        while let Some(entry) = stdin.next_entry()? {
            process_entry(
                entry,
                &err,
                compiled_specs.iter(),
                &mut copy_buf,
                &mut symlink_target,
                &mut deduped_concat_writers,
                &mut matching_handles,
            )?;
        }
    }

    for p in zip_paths.into_iter() {
        writeln!(
            &mut err.borrow_mut(),
            "extracting from zip input file {p:?}",
        )
        .unwrap();
        let zip = fs::File::open(&p)
            .wrap_err_with(|| format!("failed to open zip input file path {p:?}"))
            .and_then(|f| {
                ZipArchive::new(f)
                    .wrap_err_with(|| format!("failed to create zip archive for file {p:?}"))
            })?;
        let mut zip_entries = ZipFileInput::new(Box::new(zip));

        while let Some(entry) = zip_entries.next_entry()? {
            process_entry(
                entry,
                &err,
                compiled_specs.iter(),
                &mut copy_buf,
                &mut symlink_target,
                &mut deduped_concat_writers,
                &mut matching_handles,
            )?;
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
