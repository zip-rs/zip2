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
pub mod receiver;
pub mod transform;
use entries::{IterateEntries, StreamInput, ZipFileInput};
use matcher::EntryMatcher;
use receiver::{CompiledEntrySpec, ConcatEntry, EntryData, EntryKind, EntryReceiver, ExtractEntry};
use transform::NameTransformer;

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
        (data.kind, data.size)
    };
    if !matches!(kind, EntryKind::Symlink) {
        return Ok(None);
    }

    /* We can't read the entry name from EntryData because we can't have any immutable
     * references to ZipFileData like the name at the same time we use the entry as
     * a reader! That means our log message here is very unclear! */
    writeln!(&mut err.borrow_mut(), "reading symlink target").unwrap();
    symlink_target.clear();
    entry
        .read_to_end(symlink_target)
        .wrap_err("failed to read symlink target from zip archive entry")?;
    debug_assert_eq!(symlink_target.len(), size.try_into().unwrap());
    Ok(Some(symlink_target))
}

fn process_entry<'a, 'w, 'it>(
    mut entry: ZipFile<'a>,
    err: &Rc<RefCell<impl Write>>,
    compiled_specs: impl Iterator<Item = &'it CompiledEntrySpec<'w>>,
    copy_buf: &mut [u8],
    symlink_target: &mut Vec<u8>,
    matching_concats: &mut Vec<Rc<RefCell<dyn Write + 'w>>>,
    deduped_concat_writers: &mut Vec<Rc<RefCell<dyn Write + 'w>>>,
    matching_handles: &mut Vec<Box<dyn Write>>,
) -> Result<(), CommandError>
where
    'w: 'it,
{
    deduped_concat_writers.clear();
    matching_handles.clear();

    let symlink_target = maybe_process_symlink(&mut entry, err, symlink_target)?;
    /* We dropped any mutable handles to the entry, so now we can access its metadata again. */
    let data = EntryData::from_entry(&entry);

    let mut matching_extracts: Vec<(Cow<'_, str>, Rc<dyn EntryReceiver>)> = Vec::new();
    for spec in compiled_specs {
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
                    writeln!(&mut err.borrow_mut(), "{data:?}").unwrap();
                    writeln!(&mut err.borrow_mut(), "{new_name:?}").unwrap();
                    matching_extracts.push((new_name, recv.clone()));
                }
            }
        }
    }
    if matching_concats.is_empty() && matching_extracts.is_empty() {
        return Ok(());
    }

    /* Split output handles for concat, and split generated handles by extract source and
     * name. use Rc::ptr_eq() to split, and Cow::<'s, str>::eq() with str AsRef. */
    for concat_p in matching_concats.drain(..) {
        if deduped_concat_writers
            .iter()
            .any(|p| Rc::ptr_eq(p, &concat_p))
        {
            writeln!(&mut err.borrow_mut(), "skipping repeated concat").unwrap();
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
            writeln!(&mut err.borrow_mut(), "skipping repeated extract").unwrap();
        } else {
            deduped_matching_extracts.push((name, extract_p));
        }
    }

    matching_handles.extend(
        deduped_matching_extracts
            .into_iter()
            .map(|(name, recv)| {
                recv.generate_entry_handle(data, symlink_target.as_ref().map(|t| t.as_ref()), name)
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten(),
    );

    /* let mut derefed_concat_writers: Vec<RefMut<'_, dyn Write>> = deduped_concat_writers */
    /*     .drain(..) */
    /*     .map(|w| w.borrow_mut()) */
    /*     .collect(); */
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

    let compiled_specs =
        receiver::process_entry_and_output_specs(err.clone(), entry_specs, output_specs)?;

    let mut copy_buf: Vec<u8> = vec![0u8; 1024 * 16];
    let mut symlink_target: Vec<u8> = Vec::new();

    let mut matching_concats: Vec<Rc<RefCell<dyn Write>>> = Vec::new();
    /* let mut matching_extracts: Vec<(Cow<'_, str>, Rc<dyn EntryReceiver>)> = Vec::new(); */
    let mut deduped_concat_writers: Vec<Rc<RefCell<dyn Write>>> = Vec::new();
    /* let mut deduped_matching_extracts: Vec<(Cow<'_, str>, Rc<dyn EntryReceiver>)> = Vec::new(); */
    let mut matching_handles: Vec<Box<dyn Write>> = Vec::new();

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
                &mut matching_concats,
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
                &mut matching_concats,
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
