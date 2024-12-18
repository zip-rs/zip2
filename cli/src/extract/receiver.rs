use std::{
    borrow::Cow,
    cell::RefCell,
    fmt, fs,
    io::{self, Write},
    mem,
    path::{Path, PathBuf},
    rc::Rc,
};

use zip::{
    extra_fields::{ExtendedTimestamp, ExtraField},
    read::ZipFile,
    CompressionMethod, DateTime,
};

use super::matcher::{CompiledMatcher, EntryMatcher};
use super::transform::{CompiledTransformer, NameTransformer};
use crate::{CommandError, WrapCommandErr};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntryData<'a> {
    pub name: &'a str,
    pub kind: EntryKind,
    pub compression: CompressionMethod,
    pub unix_mode: Option<u32>,
    pub comment: &'a str,
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub local_header_start: u64,
    pub content_start: u64,
    pub central_header_start: u64,
    pub crc32: u32,
    pub last_modified_time: Option<DateTime>,
    pub extended_timestamp: Option<ExtendedTimestamp>,
}

impl<'a> EntryData<'a> {
    #[inline(always)]
    pub fn from_entry<'b>(entry: &'a ZipFile<'b>) -> Self {
        Self {
            name: entry.name(),
            kind: if entry.is_dir() {
                EntryKind::Dir
            } else if entry.is_symlink() {
                EntryKind::Symlink
            } else {
                EntryKind::File
            },
            compression: entry.compression(),
            unix_mode: entry.unix_mode(),
            comment: entry.comment(),
            uncompressed_size: entry.size(),
            compressed_size: entry.compressed_size(),
            local_header_start: entry.header_start(),
            content_start: entry.data_start(),
            central_header_start: entry.central_header_start(),
            crc32: entry.crc32(),
            last_modified_time: entry.last_modified(),
            extended_timestamp: entry
                .extra_data_fields()
                .find_map(|f| match f {
                    ExtraField::ExtendedTimestamp(ts) => Some(ts),
                })
                .cloned(),
        }
    }

    #[inline(always)]
    pub const fn content_end(&self) -> u64 {
        self.content_start + self.compressed_size
    }
}

pub struct ConcatEntry<'w> {
    pub matcher: Option<CompiledMatcher>,
    pub stream: Rc<RefCell<dyn Write + 'w>>,
}

impl<'w> fmt::Debug for ConcatEntry<'w> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "ConcatEntry {{ matcher: {:?}, stream: {:p} }}",
            &self.matcher, &self.stream
        )
    }
}

impl<'w> ConcatEntry<'w> {
    pub fn do_match<'a>(&self, data: &EntryData<'a>) -> Option<&Rc<RefCell<dyn Write + 'w>>> {
        if self
            .matcher
            .as_ref()
            .map(|m| m.matches(data))
            .unwrap_or(true)
        {
            Some(&self.stream)
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub struct ExtractEntry<'w> {
    pub matcher: Option<CompiledMatcher>,
    pub transforms: Option<CompiledTransformer>,
    pub recv: Rc<dyn EntryReceiver + 'w>,
}

impl<'w> ExtractEntry<'w> {
    pub fn do_match_and_transform<'a>(
        &self,
        data: &EntryData<'a>,
    ) -> Option<(Cow<'a, str>, &Rc<dyn EntryReceiver + 'w>)> {
        if self
            .matcher
            .as_ref()
            .map(|m| m.matches(data))
            .unwrap_or(true)
        {
            let new_name = self
                .transforms
                .as_ref()
                .map(|t| t.transform_name(data.name))
                .unwrap_or_else(|| Cow::Borrowed(data.name));
            Some((new_name, &self.recv))
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub enum CompiledEntrySpec<'w> {
    Concat(ConcatEntry<'w>),
    Extract(ExtractEntry<'w>),
}

impl<'w> CompiledEntrySpec<'w> {
    pub fn try_match_and_transform<'a>(
        &self,
        data: &EntryData<'a>,
    ) -> Option<MatchingEntrySpec<'a, '_, 'w>> {
        match self {
            Self::Concat(c) => c.do_match(data).map(MatchingEntrySpec::Concat),
            Self::Extract(e) => e
                .do_match_and_transform(data)
                .map(|(n, p)| MatchingEntrySpec::Extract(n, p)),
        }
    }
}

pub enum MatchingEntrySpec<'a, 'c, 'w> {
    Concat(&'c Rc<RefCell<dyn Write + 'w>>),
    Extract(Cow<'a, str>, &'c Rc<dyn EntryReceiver + 'w>),
}

impl<'a, 'c, 'w> MatchingEntrySpec<'a, 'c, 'w> {
    /* Split output handles for concat, and split generated handles by extract source and
     * name. use Rc::ptr_eq() to split, and Cow::<'s, str>::eq() with str AsRef. */
    pub fn is_nested_duplicate(
        self,
        deduped_concat_writers: &mut Vec<&'c Rc<RefCell<dyn Write + 'w>>>,
        deduped_matching_extracts: &mut Vec<(&'c Rc<dyn EntryReceiver + 'w>, Vec<Cow<'a, str>>)>,
    ) -> bool {
        match self {
            MatchingEntrySpec::Concat(concat_writer) => {
                if deduped_concat_writers
                    .iter()
                    .any(|p| Rc::ptr_eq(p, &concat_writer))
                {
                    true
                } else {
                    deduped_concat_writers.push(concat_writer);
                    false
                }
            }
            MatchingEntrySpec::Extract(name, extract_receiver) => {
                if let Some((_, names)) = deduped_matching_extracts
                    .iter_mut()
                    .find(|(p, _)| Rc::ptr_eq(p, &extract_receiver))
                {
                    if names.iter().any(|n| n.as_ref() == name.as_ref()) {
                        true
                    } else {
                        names.push(name);
                        false
                    }
                } else {
                    deduped_matching_extracts.push((extract_receiver, vec![name]));
                    false
                }
            }
        }
    }
}

pub trait EntryReceiver: fmt::Debug {
    fn generate_entry_handle<'s>(
        &self,
        data: &EntryData<'s>,
        symlink_target: Option<&[u8]>,
        name: Cow<'s, str>,
    ) -> Result<Option<Box<dyn Write>>, CommandError>;

    fn finalize_entries(&self) -> Result<(), CommandError>;
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg(unix)]
struct PermsEntry {
    path: PathBuf,
    mode: u32,
}

pub struct FilesystemReceiver<W> {
    err: Rc<RefCell<W>>,
    output_dir: PathBuf,
    #[cfg(unix)]
    perms_to_set: RefCell<Vec<PermsEntry>>,
}

impl<W> FilesystemReceiver<W> {
    pub fn new(err: Rc<RefCell<W>>, output_dir: PathBuf) -> Self {
        Self {
            err,
            output_dir,
            #[cfg(unix)]
            perms_to_set: RefCell::new(Vec::new()),
        }
    }
}

impl<W> fmt::Debug for FilesystemReceiver<W> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "FilesystemReceiver {{ output_dir: {:?} }}",
            &self.output_dir
        )
    }
}

impl<W> FilesystemReceiver<W>
where
    W: Write,
{
    #[cfg(unix)]
    fn create_or_overwrite_symlink(
        err: &mut impl Write,
        target: &[u8],
        full_output_path: &Path,
    ) -> Result<(), CommandError> {
        use std::{
            ffi::OsStr,
            os::unix::{ffi::OsStrExt, fs::symlink},
        };
        let target = OsStr::from_bytes(target);
        writeln!(err, "entry is symlink to {target:?}, creating").unwrap();
        /* The stdlib symlink function has no functionality like OpenOptions to
         * truncate a symlink if it already exists, so we have to do that ourselves
         * here. */
        if let Err(e) = symlink(target, full_output_path) {
            let e = match e.kind() {
                io::ErrorKind::AlreadyExists => {
                    writeln!(err, "a file already existed at the symlink target {full_output_path:?}, removing")
                                    .unwrap();
                    fs::remove_file(full_output_path).wrap_err_with(|| {
                        format!("failed to remove file at symlink target {full_output_path:?}")
                    })?;
                    writeln!(
                        err,
                        "successfully removed file entry, creating symlink again"
                    )
                    .unwrap();
                    symlink(target, full_output_path).err()
                }
                _ => Some(e),
            };
            if let Some(e) = e {
                return Err(e).wrap_err_with(|| {
                    format!(
                        "failed to create symlink at {full_output_path:?} with target {target:?}"
                    )
                });
            }
        }
        Ok(())
    }
}

impl<W> EntryReceiver for FilesystemReceiver<W>
where
    W: Write,
{
    fn generate_entry_handle<'s>(
        &self,
        data: &EntryData<'s>,
        symlink_target: Option<&[u8]>,
        name: Cow<'s, str>,
    ) -> Result<Option<Box<dyn Write>>, CommandError> {
        let mut err = self.err.borrow_mut();
        let full_output_path = self.output_dir.join(name.as_ref());
        writeln!(
            err,
            "receiving entry {} with name {name} and writing to path {full_output_path:?}",
            data.name
        )
        .unwrap();

        match data.kind {
            EntryKind::Dir => {
                writeln!(err, "entry is directory, creating").unwrap();
                fs::create_dir_all(&full_output_path).wrap_err_with(|| {
                    format!("failed to create directory entry at {full_output_path:?}")
                })?;
            }
            EntryKind::Symlink => {
                let target = symlink_target.expect("we should have generated this");

                #[cfg(unix)]
                Self::create_or_overwrite_symlink(&mut *err, target, &full_output_path)?;
                #[cfg(not(unix))]
                todo!("TODO: cannot create symlink for entry {name} on non-unix yet!");
            }
            EntryKind::File => {
                writeln!(err, "entry is file, creating").unwrap();
                if let Some(containing_dir) = full_output_path.parent() {
                    fs::create_dir_all(containing_dir).wrap_err_with(|| {
                        format!("failed to create parent dirs for file at {full_output_path:?}")
                    })?;
                } else {
                    writeln!(err, "entry had no parent dir (in root dir?)").unwrap();
                }
                let outfile = fs::File::create(&full_output_path)
                    .wrap_err_with(|| format!("failed to create file at {full_output_path:?}"))?;
                return Ok(Some(Box::new(outfile)));
            }
        }

        #[cfg(unix)]
        if let Some(mode) = data.unix_mode {
            writeln!(
                err,
                "storing unix mode {mode} for path {full_output_path:?}"
            )
            .unwrap();
            self.perms_to_set.borrow_mut().push(PermsEntry {
                path: full_output_path,
                mode,
            });
        }

        Ok(None)
    }

    fn finalize_entries(&self) -> Result<(), CommandError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut perms_to_set = mem::take(&mut *self.perms_to_set.borrow_mut());
            perms_to_set.sort_unstable();
            writeln!(
                &mut self.err.borrow_mut(),
                "perms to set (these are done in reverse order): {perms_to_set:?}"
            )
            .unwrap();
            for PermsEntry { path, mode } in perms_to_set.into_iter().rev() {
                let perms = fs::Permissions::from_mode(mode);
                fs::set_permissions(&path, perms.clone())
                    .wrap_err_with(|| format!("error setting perms {perms:?} for path {path:?}"))?;
            }
        }
        Ok(())
    }
}
