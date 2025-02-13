//! Prepare the output directory structure for extraction and pre-allocate file handles.

use by_address::ByAddress;

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use crate::read::pipelining::path_splitting::{DirEntry, FSEntry};
use crate::types::ZipFileData;

/* TODO: figure out how to handle symlinks! These are especially difficult because:
 * (1) windows symlinks files and directories differently, and only on newer windows versions,
 * (2) later entries in the zip may refer to symlink paths from earlier in the zip.
 *
 * Of these issues, (2) is more difficult and intrinsic to the problem space. In order to
 * correctly extract symlinks in a pipelined/parallel fashion, we need to identify these order
 * dependencies and schedule the symlink dereference (reading the target value from the zip)
 * before we create any directories or allocate any output file handles that dereference that
 * symlink. This is less of a problem with the synchronous in-order extraction because it
 * creates any symlinks immediately (it imposes a total ordering dependency over all entries).
 *
 * Note: futures::future::Shared is ideal for a dependency DAG of shared tasks like this:
 *       https://docs.rs/futures/latest/futures/future/struct.Shared.html
 */
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) struct AllocatedHandles<'a> {
    pub file_handle_mapping: HashMap<ByAddress<Pin<&'a ZipFileData>>, fs::File>,
    pub perms_todo: Vec<(PathBuf, fs::Permissions)>,
}

#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) fn transform_entries_to_allocated_handles<'a>(
    top_level_extraction_dir: &Path,
    lex_entry_trie: impl IntoIterator<Item = (&'a str, FSEntry<'a, &'a ZipFileData>)>,
) -> Result<AllocatedHandles<'a>, io::Error> {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    /* TODO: we create subdirs by constructing path strings, which may fail at overlarge
     * paths. This may be fixable on unix with mkdirat()/openat(), but would require more
     * complex platform-specific programming. However, the result would likely decrease the
     * number of syscalls, which may also improve performance. It may also be slightly easier to
     * follow the logic if we can refer to directory inodes instead of constructing path strings
     * as a proxy. This should be considered if requested by users. */
    fs::create_dir_all(top_level_extraction_dir)?;

    #[allow(clippy::mutable_key_type)]
    let mut file_handle_mapping: HashMap<ByAddress<Pin<&'a ZipFileData>>, fs::File> =
        HashMap::new();
    let mut entry_queue: VecDeque<(PathBuf, FSEntry<'a, &'a ZipFileData>)> = lex_entry_trie
        .into_iter()
        .map(|(entry_name, entry_data)| (top_level_extraction_dir.join(entry_name), entry_data))
        .collect();
    let mut perms_todo: Vec<(PathBuf, fs::Permissions)> = Vec::new();

    while let Some((path, entry)) = entry_queue.pop_front() {
        match entry {
            FSEntry::File(data) => {
                let key = ByAddress(Pin::new(data));

                #[cfg_attr(not(unix), allow(unused_variables))]
                if let Some(mode) = data.unix_mode() {
                    /* TODO: consider handling the readonly bit on windows. We don't currently
                     * do this in normal extraction, so we don't need to do this yet for
                     * pipelining. */

                    /* Write the desired perms to the perms queue. */
                    #[cfg(unix)]
                    perms_todo.push((path.clone(), fs::Permissions::from_mode(mode)));
                }

                let handle = fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(path)?;
                assert!(file_handle_mapping.insert(key, handle).is_none());
            }
            #[cfg_attr(not(unix), allow(unused_variables))]
            FSEntry::Dir(DirEntry {
                properties,
                children,
            }) => {
                /* FIXME: use something like make_writable_dir_all() and then add to
                 * perms_todo! */
                match fs::create_dir(&path) {
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                    Err(e) => return Err(e),
                    Ok(()) => (),
                }

                /* (1) Write any desired perms to the perms queue. */
                #[cfg(unix)]
                if let Some(perms_to_set) = properties.and_then(|data| data.unix_mode()) {
                    perms_todo.push((path.clone(), fs::Permissions::from_mode(perms_to_set)));
                }
                /* (2) Generate sub-entries by constructing full paths. */
                for (sub_name, entry) in children.into_iter() {
                    let full_name = path.join(sub_name);
                    entry_queue.push_back((full_name, entry));
                }
            }
        }
    }

    /* NB: Iterate in *REVERSE* so that child directories are set before parents! Setting
     * a parent readonly would stop us from setting child perms. */
    perms_todo.reverse();

    Ok(AllocatedHandles {
        file_handle_mapping,
        perms_todo,
    })
}

#[cfg(test)]
mod test {
    use tempdir::TempDir;

    use std::io::{prelude::*, Cursor};

    use crate::write::{SimpleFileOptions, ZipWriter};

    use super::*;
    use crate::read::pipelining::path_splitting::lexicographic_entry_trie;

    #[test]
    fn subdir_creation() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        /* Create test archive. */
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default();

        zip.start_file("a/b/c", opts.unix_permissions(0o765))
            .unwrap();
        zip.write_all(b"asdf").unwrap();

        zip.add_directory("a/b", opts.unix_permissions(0o500))
            .unwrap();

        /* Create readable archive and extraction dir. */
        let zip = zip.finish_into_readable().unwrap();
        let td = TempDir::new("pipeline-test").unwrap();

        /* (1) Create lex entry trie. */
        let trie = lexicographic_entry_trie(
            zip.shared
                .files
                .iter()
                .map(|(name, data)| (name.as_ref(), data)),
        )
        .unwrap();

        /* (2) Generate handles. */
        #[cfg_attr(not(unix), allow(unused_variables))]
        let AllocatedHandles {
            file_handle_mapping,
            perms_todo,
        } = transform_entries_to_allocated_handles(td.path(), trie).unwrap();

        let mut files: Vec<_> = file_handle_mapping.into_iter().collect();
        assert_eq!(1, files.len());
        let (file_data, mut file) = files.pop().unwrap();
        assert_eq!(
            file_data,
            ByAddress(Pin::new(zip.shared.files.get_index(0).unwrap().1))
        );
        /* We didn't write anything to the file, so it's still empty at this point. */
        assert_eq!(b"", &fs::read(td.path().join("a/b/c")).unwrap()[..]);
        file.write_all(b"asdf").unwrap();
        file.sync_data().unwrap();
        /* Now the data is synced! */
        assert_eq!(b"asdf", &fs::read(td.path().join("a/b/c")).unwrap()[..]);

        #[cfg(unix)]
        assert_eq!(
            perms_todo
                .into_iter()
                .map(|(path, perms)| (path, perms.mode() & 0o777))
                .collect::<Vec<_>>(),
            vec![
                (td.path().join("a/b/c"), 0o765),
                (td.path().join("a/b"), 0o500),
            ]
        );
    }
}
