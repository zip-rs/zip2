//! Extraction

use crate::read::stream::{ZipStreamReader, ZipStreamVisitor};
use crate::read::{ZipFile, ZipFileEntry};
use crate::result::{ZipResult, invalid};
use crate::{ZipArchive, result::ZipError};
use std::ffi::OsStr;
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

/// Upper bound on the uncompressed size of a symlink entry.
///
/// A symlink target is a filesystem path, so its length is bounded by the
/// platform (`PATH_MAX` is 4096 on Linux) no matter what the archive says. The
/// uncompressed size stored in the central directory is attacker-controlled,
/// so entries above this bound are rejected rather than pre-allocated.
pub(crate) const MAX_SYMLINK_TARGET_LEN: u64 = 4096;

/// Symlink extract action
#[derive(Debug, Default, Copy, Clone)]
pub enum SymlinkExtractAction {
    ///  Omit symlinks.
    NoExtract,
    /// Convert symlinks to text file
    ExtractAsFile,
    /// Convert symlinks to text file as utf8
    #[cfg_attr(not(any(unix, windows)), default)]
    ExtractAsFileUtf8,
    /// Extract symlinks if the recursive target is in the destination folder (default on Unix and Windows, but not possible on other platforms).
    #[cfg(any(unix, windows))]
    #[default]
    ExtractRecursiveInFolder,
    /// Extract symlinks if the _direct_ target is in the destination folder (i.e. it can point to a pre-existing symlink that in turn points outside the folder).
    #[cfg(any(unix, windows))]
    ExtractInFolder,
    /// Extract symlinks with no restrictions.
    ///
    /// On Windows, creating a symlink requires that we know whether the target is a file or directory.
    /// To determine that, this option will open the file metadata of the target path, which will
    /// cause NTLMv2 authentication to be sent if the target is on an SMB network share. This may be a security risk,
    /// because it allows relay attacks and offline cracking. WebDAV is also vulnerable, but only when a file
    /// host in the Intranet Zone is already compromised.
    #[cfg(any(unix, windows))]
    ExtractNoRestrictions,
}

/// Extract options
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct ExtractOptions {
    pub(crate) symlink_action: SymlinkExtractAction,
    pub(crate) size_limit: Option<u64>,
}

impl ExtractOptions {
    /// Set the symlink action
    #[must_use]
    pub fn symlink_action(self, symlink_action: SymlinkExtractAction) -> Self {
        Self {
            symlink_action,
            ..self
        }
    }

    /// Set the maximum declared uncompressed size accepted for extraction.
    #[must_use]
    pub fn with_size_limit(self, size_limit: u64) -> Self {
        Self {
            size_limit: Some(size_limit),
            ..self
        }
    }
}

pub(crate) use crate::path::make_writable_dir_all;

#[cfg(unix)]
pub(crate) fn make_symlink_impl<T>(
    outpath: &Path,
    target_str: &str,
    _existing_files: &IndexMap<Box<[u8]>, T>,
) -> ZipResult<()> {
    std::os::unix::fs::symlink(Path::new(&target_str), outpath)?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn make_symlink_impl<T>(
    outpath: &Path,
    target_str: &str,
    existing_files: &IndexMap<Box<[u8]>, T>,
) -> ZipResult<()> {
    use crate::format::functions::is_dir;
    let target = Path::new(OsStr::new(&target_str));
    let target_is_dir_from_archive =
        is_dir(target_str.as_bytes()) && existing_files.contains_key(target_str.as_bytes());
    let target_is_dir = if target_is_dir_from_archive {
        true
    } else if let Ok(meta) = std::fs::metadata(target) {
        meta.is_dir()
    } else {
        false
    };
    if target_is_dir {
        std::os::windows::fs::symlink_dir(target, outpath)?;
    } else {
        std::os::windows::fs::symlink_file(target, outpath)?;
    }
    Ok(())
}

#[cfg(any(windows, unix))]
pub(crate) fn make_symlink<T>(
    base: &Path, // canonicalized destination directory
    outpath: &Path,
    target: &[u8],
    #[cfg_attr(not(any(windows, unix)), allow(unused))] existing_files: &IndexMap<Box<[u8]>, T>,
    symlink_action: SymlinkExtractAction,
) -> ZipResult<()> {
    let Ok(target_str) = std::str::from_utf8(target) else {
        return Err(invalid!("Invalid UTF-8 as symlink target"));
    };

    // Check where the symlink would resolve to before creating it. A relative target
    // is resolved from the parent of the symlink itself, exactly as the OS would.
    let target_path = Path::new(target_str);
    let (start, rest) = if target_path.is_absolute() {
        let Some(rest) = crate::path::strip_base_prefix(base, target_path) else {
            return Err(invalid!(
                "make_symlink: Symlink target escapes the destination directory"
            ));
        };
        (base, rest)
    } else {
        // A Windows target such as `C:foo` (relative to the current directory of a
        // drive) or `\foo` (relative to the root of the current drive) is neither
        // absolute nor safely relative: the OS resolves it from somewhere other than
        // the parent of the symlink, so there is nothing meaningful to check here.
        if target_path.components().any(|component| {
            matches!(
                component,
                std::path::Component::Prefix(_) | std::path::Component::RootDir
            )
        }) {
            return Err(invalid!("Invalid symlink target path"));
        }
        let parent = outpath.parent().unwrap_or(base);
        if !parent.starts_with(base) {
            return Err(invalid!("Symlink is not inside the destination directory"));
        }
        (parent, target_path)
    };

    // ExtractNoRestrictions is allowed outside (no restrictions) - but it uses
    // [`make_symlink_impl`] directly
    // ExtractRecursiveInFolder is allowed outside (it will be checked that the symlink fall in the
    // folder)
    #[cfg(any(windows, unix))]
    let allow_outside = matches!(
        symlink_action,
        SymlinkExtractAction::ExtractNoRestrictions
            | SymlinkExtractAction::ExtractRecursiveInFolder
    );
    #[cfg(not(any(windows, unix)))]
    let allow_outside = false;
    if allow_outside {
        // For ExtractRecursiveInFolder: the literal path may leave `base` (e.g.
        // `../sibling`), but it must canonicalize (following real filesystem symlinks) back into `base`.
        #[cfg(any(unix, windows))]
        if matches!(
            symlink_action,
            SymlinkExtractAction::ExtractRecursiveInFolder
        ) {
            // Try to canonicalize the resolved path so that symlinks in the filesystem
            // that point back inside `base` are followed.
            let abs_to_canonicalize = if target_path.is_absolute() {
                target_path
            } else {
                &start.join(target_path)
            };
            match std::fs::canonicalize(abs_to_canonicalize) {
                Ok(canonical) => {
                    if !canonical.starts_with(base) {
                        return Err(invalid!("Path escapes the destination directory"));
                    }
                }
                Err(_) => {
                    crate::path::resolve_enclosed(
                        base,
                        start,
                        rest.components().map(|c| c.as_os_str().to_os_string()),
                        false,
                    )?;
                }
            }
        }
    } else {
        crate::path::resolve_enclosed(
            base,
            start,
            rest.components().map(|c| c.as_os_str().to_os_string()),
            false, // only checking here, so nothing is created
        )?;
    }

    make_symlink_impl(outpath, target_str, existing_files)
}

/// Wasm and others
#[cfg(not(any(windows, unix)))]
pub(crate) fn make_symlink<T>(
    // No symlink is created on these targets, so there is nothing to contain.
    _base: &Path,
    outpath: &Path,
    target: &[u8],
    #[cfg_attr(not(any(windows, unix)), allow(unused))] existing_files: &IndexMap<Box<[u8]>, T>,
    _symlink_action: SymlinkExtractAction,
) -> ZipResult<()> {
    make_symlink_as_file_utf8(outpath, target)?;
    Ok(())
}

pub(crate) fn make_symlink_as_file_utf8(outpath: &Path, target: &[u8]) -> ZipResult<()> {
    let Ok(_) = std::str::from_utf8(target) else {
        return Err(invalid!("Invalid UTF-8 as symlink target"));
    };
    make_symlink_as_file(outpath, target)?;
    Ok(())
}

pub(crate) fn make_symlink_as_file(outpath: &Path, target: &[u8]) -> std::io::Result<()> {
    use std::fs::File;
    use std::io::Write;
    let output = File::create(outpath);
    output?.write_all(target)?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn set_permissions_no_follow_symlink(
    path: &Path,
    mode: std::fs::Permissions,
) -> std::io::Result<()> {
    use std::fs;
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        {
            // These are the only platforms where permission bits on symlinks themselves are meaningful.

            use nix::fcntl::AT_FDCWD;
            use nix::sys::stat::{FchmodatFlags, Mode, fchmodat};
            use std::os::unix::fs::PermissionsExt;

            // `mode_t` is `u16` on macOS, FreeBSD and DragonFly, but `u32` on NetBSD and OpenBSD.
            let mode = Mode::from_bits_truncate(mode.mode() as nix::libc::mode_t);

            // Use fchmodat with the flag telling it NOT to follow the symlink
            fchmodat(AT_FDCWD, path, mode, FchmodatFlags::NoFollowSymlink)?;
        }
    } else {
        fs::set_permissions(path, mode)?;
    }
    Ok(())
}

/// Store all entries which specify a numeric "mode" which is familiar to POSIX operating systems.
#[derive(Default, Debug)]
struct UnixFileModes {
    #[cfg(unix)]
    map: std::collections::BTreeMap<std::path::PathBuf, u32>,
    #[cfg(not(unix))]
    read_only_set: std::collections::BTreeSet<std::path::PathBuf>,
}

impl UnixFileModes {
    pub fn add_mode(&mut self, path: std::path::PathBuf, mode: u32) {
        // We don't print a warning or consider it remotely out of the ordinary to receive two
        // separate modes for the same path (e.g. entries named `a` and `./a`): just take the
        // later one, as the later file's contents are what's on disk.
        #[cfg(unix)]
        self.map.insert(path, mode);
        #[cfg(not(unix))]
        if mode & 0o222 == 0 {
            self.read_only_set.insert(path);
        } else {
            self.read_only_set.remove(&path);
        }
    }

    #[cfg(unix)]
    // Child nodes will be sorted later lexicographically, so reversing the order puts them first.
    pub fn all_perms_with_children_first(
        self,
    ) -> impl IntoIterator<Item = (std::path::PathBuf, std::fs::Permissions)> {
        use std::os::unix::fs::PermissionsExt;
        self.map
            .into_iter()
            .rev()
            .map(|(p, m)| (p, std::fs::Permissions::from_mode(m)))
    }

    #[cfg(not(unix))]
    // Child nodes will be sorted later lexicographically, so reversing the order puts them first.
    pub fn all_read_only_with_children_first(self) -> impl IntoIterator<Item = std::path::PathBuf> {
        self.read_only_set.into_iter().rev()
    }
}

impl<R: Read + Seek> ZipArchive<R> {
    /// Extract a Zip archive into a directory, overwriting files if they
    /// already exist. Paths are sanitized with [`ZipFile::enclosed_name`](crate::read::ZipFile::enclosed_name).
    ///
    /// Symbolic links are only created, and only followed, if their target resolves to a path
    /// inside the destination directory. Paths are resolved one component at a time, and the
    /// target of every symlink encountered is resolved in the same way, so a symlink cannot be
    /// used as an intermediate component to reach outside the destination directory. Note that
    /// this does not protect against another process concurrently modifying the destination
    /// directory during extraction.
    ///
    /// Extraction is not atomic. If an error is encountered, some of the files
    /// may be left on disk. However, on Unix targets, no newly-created directories with part but
    /// not all of their contents extracted will be readable, writable or usable as process working
    /// directories by any non-root user except you.
    ///
    /// On Unix and Windows, symbolic links are extracted correctly. On other platforms such as
    /// WebAssembly, symbolic links aren't supported, so they're extracted as normal files
    /// containing the target path in UTF-8.
    pub fn extract<P: AsRef<Path>>(&mut self, directory: P) -> ZipResult<()> {
        self.extract_internal(
            directory,
            None::<fn(&Path) -> bool>,
            ExtractOptions::default(),
        )
    }
    /// Same as extract but with options
    pub fn extract_with_options<P: AsRef<Path>>(
        &mut self,
        directory: P,
        extract_options: ExtractOptions,
    ) -> ZipResult<()> {
        self.extract_internal(directory, None::<fn(&Path) -> bool>, extract_options)
    }

    /// Extracts a Zip archive into a directory in the same fashion as
    /// [`ZipArchive::extract`], but detects a "root" directory in the archive
    /// (a single top-level directory that contains the rest of the archive's
    /// entries) and extracts its contents directly.
    ///
    /// For a sensible default `filter`, you can use [`crate::read::root_dir_common_filter`].
    /// For a custom `filter`, see [`RootDirFilter`].
    ///
    /// See [`ZipArchive::root_dir`] for more information on how the root
    /// directory is detected and the meaning of the `filter` parameter.
    ///
    /// ## Example
    ///
    /// Imagine a Zip archive with the following structure:
    ///
    /// ```text
    /// root/file1.txt
    /// root/file2.txt
    /// root/sub/file3.txt
    /// root/sub/subsub/file4.txt
    /// ```
    ///
    /// If the archive is extracted to `foo` using [`ZipArchive::extract`],
    /// the resulting directory structure will be:
    ///
    /// ```text
    /// foo/root/file1.txt
    /// foo/root/file2.txt
    /// foo/root/sub/file3.txt
    /// foo/root/sub/subsub/file4.txt
    /// ```
    ///
    /// If the archive is extracted to `foo` using
    /// [`ZipArchive::extract_unwrapped_root_dir`], the resulting directory
    /// structure will be:
    ///
    /// ```text
    /// foo/file1.txt
    /// foo/file2.txt
    /// foo/sub/file3.txt
    /// foo/sub/subsub/file4.txt
    /// ```
    ///
    /// ## Example - No Root Directory
    ///
    /// Imagine a Zip archive with the following structure:
    ///
    /// ```text
    /// root/file1.txt
    /// root/file2.txt
    /// root/sub/file3.txt
    /// root/sub/subsub/file4.txt
    /// other/file5.txt
    /// ```
    ///
    /// Due to the presence of the `other` directory,
    /// [`ZipArchive::extract_unwrapped_root_dir`] will extract this in the same
    /// fashion as [`ZipArchive::extract`] as there is now no "root directory."
    pub fn extract_unwrapped_root_dir<P: AsRef<Path>>(
        &mut self,
        directory: P,
        root_dir_filter: impl RootDirFilter,
    ) -> ZipResult<()> {
        self.extract_internal(directory, Some(root_dir_filter), ExtractOptions::default())
    }

    /// Find the "root directory" of an archive if it exists, filtering out
    /// irrelevant entries when searching.
    ///
    /// Our definition of a "root directory" is a single top-level directory
    /// that contains the rest of the archive's entries. This is useful for
    /// extracting archives that contain a single top-level directory that
    /// you want to "unwrap" and extract directly.
    ///
    /// For a sensible default filter, you can use [`crate::read::root_dir_common_filter`].
    /// For a custom filter, see [`RootDirFilter`].
    pub fn root_dir(&self, filter: impl RootDirFilter) -> ZipResult<Option<PathBuf>> {
        let mut root_dir: Option<PathBuf> = None;

        for i in 0..self.len() {
            let (file_name_raw, file) = self
                .shared
                .files
                .get_index(i)
                .ok_or(ZipError::FileNotFound)?;
            let file_name = match file.name(file_name_raw) {
                Ok(file_name) => file_name,
                Err(_) => continue,
            };
            let path = match file.enclosed_name(&file_name) {
                Some(path) => path,
                None => return Ok(None),
            };

            if !filter(&path) {
                continue;
            }

            macro_rules! replace_root_dir {
                ($path:ident) => {
                    match &mut root_dir {
                        Some(root_dir) => {
                            if *root_dir != $path {
                                // We've found multiple root directories,
                                // abort.
                                return Ok(None);
                            }
                            continue;
                        }

                        None => {
                            root_dir = Some($path.into());
                            continue;
                        }
                    }
                };
            }

            // If this entry is located at the root of the archive...
            if path.components().count() == 1 {
                if file.is_dir(file_name_raw.as_ref()) {
                    // If it's a directory, it could be the root directory.
                    replace_root_dir!(path);
                }
                // If it's anything else, this archive does not have a
                // root directory.
                return Ok(None);
            }

            // Find the root directory for this entry.
            let mut path = path.as_path();
            while let Some(parent) = path.parent().filter(|path| *path != Path::new("")) {
                path = parent;
            }

            replace_root_dir!(path);
        }

        Ok(root_dir)
    }

    fn extract_internal<P: AsRef<Path>>(
        &mut self,
        directory: P,
        root_dir_filter: Option<impl RootDirFilter>,
        extract_options: ExtractOptions,
    ) -> ZipResult<()> {
        use std::fs;

        if let Some(limit) = extract_options.size_limit {
            match self.decompressed_size() {
                Some(size) if size > u128::from(limit) => {
                    return Err(ZipError::DecompressedSizeLimitExceeded { size, limit });
                }
                None => return Err(ZipError::DecompressedSizeUnknown),
                Some(_) => {}
            }
        }

        fs::create_dir_all(&directory)?;
        let directory = directory.as_ref().canonicalize()?;

        let root_dir = root_dir_filter
            .and_then(|filter| {
                self.root_dir(&filter)
                    .transpose()
                    .map(|root_dir| root_dir.map(|root_dir| (root_dir, filter)))
            })
            .transpose()?;

        // If we have a root dir, simplify the path components to be more
        // appropriate for passing to `safe_prepare_path`
        let root_dir = root_dir
            .as_ref()
            .map(|(root_dir, filter)| {
                crate::path::simplified_components(root_dir)
                    .ok_or_else(|| {
                        // Should be unreachable
                        debug_assert!(false, "Invalid root dir path");

                        invalid!("Invalid root dir path")
                    })
                    .map(|root_dir| (root_dir, filter))
            })
            .transpose()?;

        let mut files_by_unix_mode = UnixFileModes::default();

        for i in 0..self.len() {
            let mut file = self.by_index(i)?;

            let mut outpath = directory.clone();
            /* TODO: the control flow of this method call and subsequent expectations about the
             *       values in this loop is extremely difficult to follow. It also appears to
             *       perform a nested loop upon extracting every single file entry? Why does it
             *       accept two arguments that point to the same directory path, one mutable? */
            file.safe_prepare_path(directory.as_ref(), &mut outpath, root_dir.as_ref())?;

            if file.is_symlink() {
                // `file.size()` is the uncompressed size declared in the central
                // directory, i.e. attacker-controlled. A symlink target is a
                // path, so an entry claiming more than `MAX_SYMLINK_TARGET_LEN`
                // bytes is malformed. Reject it instead of reserving -- and then
                // reading -- the declared size, which would otherwise let a
                // small archive exhaust memory or abort the process.
                let declared_len = file.size();
                if declared_len > MAX_SYMLINK_TARGET_LEN {
                    return Err(invalid!(
                        "symlink target declares {} bytes, more than the {} byte limit",
                        declared_len,
                        MAX_SYMLINK_TARGET_LEN
                    ));
                }
                let mut target = Vec::with_capacity(declared_len as usize);
                file.read_to_end(&mut target)?;
                drop(file);
                match extract_options.symlink_action {
                    SymlinkExtractAction::NoExtract => {
                        // do nothing
                    }
                    SymlinkExtractAction::ExtractAsFile => {
                        make_symlink_as_file(&outpath, &target)?;
                    }
                    SymlinkExtractAction::ExtractAsFileUtf8 => {
                        make_symlink_as_file_utf8(&outpath, &target)?;
                    }
                    #[cfg(any(unix, windows))]
                    SymlinkExtractAction::ExtractRecursiveInFolder => {
                        make_symlink(
                            directory.as_ref(),
                            &outpath,
                            &target,
                            &self.shared.files,
                            extract_options.symlink_action,
                        )?;
                    }
                    #[cfg(any(unix, windows))]
                    SymlinkExtractAction::ExtractInFolder => {
                        make_symlink(
                            directory.as_ref(),
                            &outpath,
                            &target,
                            &self.shared.files,
                            extract_options.symlink_action,
                        )?;
                    }
                    #[cfg(any(unix, windows))]
                    SymlinkExtractAction::ExtractNoRestrictions => {
                        let Ok(target_str) = std::str::from_utf8(&target) else {
                            return Err(invalid!("Invalid UTF-8 as symlink target"));
                        };
                        make_symlink_impl(&outpath, target_str, &self.shared.files)?;
                    }
                }
            } else if file.is_dir() {
                make_writable_dir_all(&outpath)?;
                // Check for real permissions, which we'll set in a second pass.
                if let Some(mode) = file.unix_mode() {
                    files_by_unix_mode.add_mode(outpath, mode);
                }
            } else {
                let mut outfile = fs::File::create(&outpath)?;
                io::copy(&mut file, &mut outfile)?;
                // Set original timestamp.
                #[cfg(feature = "chrono")]
                if let Some(last_modified) = file.last_modified()
                    && let Some(t) = last_modified.datetime_to_systemtime()
                {
                    outfile.set_modified(t)?;
                }
                #[cfg(not(unix))]
                if file.is_read_only() {
                    let mut permissions = outfile.metadata()?.permissions();
                    permissions.set_readonly(true);
                    outfile.set_permissions(permissions)?;
                }
                #[cfg(unix)]
                if let Some(mode) = file.unix_mode() {
                    use std::os::unix::fs::PermissionsExt;
                    outfile.set_permissions(fs::Permissions::from_mode(mode))?;
                }
            }
        }

        // Ensure we update children's permissions before making a parent unwritable.
        #[cfg(unix)]
        for (path, perms) in files_by_unix_mode.all_perms_with_children_first() {
            set_permissions_no_follow_symlink(&path, perms)?;
        }
        #[cfg(not(unix))]
        for path in files_by_unix_mode.all_read_only_with_children_first() {
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                let mut permissions = metadata.permissions();
                permissions.set_readonly(true);
                fs::set_permissions(path, permissions)?;
            }
        }

        Ok(())
    }
}

/// A filter that determines whether an entry should be ignored when searching
/// for the root directory of a Zip archive.
///
/// Returns `true` if the entry should be considered, and `false` if it should
/// be ignored.
///
/// See [`root_dir_common_filter`] for a sensible default filter.
pub trait RootDirFilter: Fn(&Path) -> bool {}
impl<F: Fn(&Path) -> bool> RootDirFilter for F {}

/// Common filters when finding the root directory of a Zip archive.
///
/// This filter is a sensible default for most use cases and filters out common
/// system files that are usually irrelevant to the contents of the archive.
///
/// Currently, the filter ignores:
/// - `/__MACOSX/`
/// - `/.DS_Store`
/// - `/Thumbs.db`
///
/// **This function is not guaranteed to be stable and may change in future versions.**
///
/// # Example
///
/// ```rust
/// # use std::path::Path;
/// assert!(zip::read::root_dir_common_filter(Path::new("foo.txt")));
/// assert!(!zip::read::root_dir_common_filter(Path::new(".DS_Store")));
/// assert!(!zip::read::root_dir_common_filter(Path::new("Thumbs.db")));
/// assert!(!zip::read::root_dir_common_filter(Path::new("__MACOSX")));
/// assert!(!zip::read::root_dir_common_filter(Path::new("__MACOSX/foo.txt")));
/// ```
#[must_use]
pub fn root_dir_common_filter(path: &Path) -> bool {
    const COMMON_FILTER_ROOT_FILES: &[&str] = &[".DS_Store", "Thumbs.db"];

    if path.starts_with("__MACOSX") {
        return false;
    }

    if path.components().count() == 1
        && path.file_name().is_some_and(|file_name| {
            COMMON_FILTER_ROOT_FILES
                .iter()
                .map(OsStr::new)
                .any(|cmp| cmp == file_name)
        })
    {
        return false;
    }

    true
}

impl<'a, R: Read + ?Sized> ZipFile<'a, R> {
    /// Prepare the path for extraction by creating necessary missing directories and checking for symlinks to be contained within the base path.
    ///
    /// `base_path` parameter is assumed to be canonicalized.
    pub(crate) fn safe_prepare_path(
        &self,
        base_path: &Path,
        outpath: &mut PathBuf,
        root_dir: Option<&(Vec<&OsStr>, impl RootDirFilter)>,
    ) -> ZipResult<()> {
        let file_name = self.name()?;
        let components = self
            .data
            .simplified_components(&file_name)
            .ok_or(invalid!("Invalid file path"))?;

        let components = match root_dir {
            Some((root_dir, filter)) => match components.strip_prefix(&**root_dir) {
                Some(components) => components,

                None => {
                    debug_assert!(
                        !filter(&PathBuf::from_iter(components.iter())),
                        "Root directory filter should not match at this point"
                    );

                    &components[..]
                }
            },

            None => &components[..],
        };

        *outpath = crate::path::resolve_enclosed(
            base_path,
            base_path,
            components.iter().map(|c| c.to_os_string()),
            true, // create intermediate directories
        )?;

        Ok(())
    }
}

impl<R: Read> ZipStreamReader<R> {
    /// Extract a Zip archive into a directory, overwriting files if they
    /// already exist. Paths are sanitized with [`ZipFile::enclosed_name`].
    ///
    /// Extraction is not atomic; If an error is encountered, some of the files
    /// may be left on disk. The file(s) and dir(s) are first created, then the
    /// permissions are applied to them
    ///
    /// Extraction of symlink is not possible since we don't have access to the
    /// external attributes in the local headers of the entries
    pub fn extract<P: AsRef<Path>>(self, directory: P) -> ZipResult<()> {
        /// Destination, every entry written in this pass (raw local name -> the checked output
        /// path `visit_file` wrote it to), and the modes the central directory gives them.
        struct Extractor {
            outpath: PathBuf,
            resolved_paths: IndexMap<Box<[u8]>, PathBuf>,
            file_permissions: std::collections::BTreeMap<PathBuf, u32>,
        }
        impl ZipStreamVisitor for Extractor {
            fn visit_file<R: Read>(&mut self, file: &mut ZipFile<'_, R>) -> ZipResult<()> {
                let extract_options = ExtractOptions::default();
                let mut outpath = self.outpath.clone();
                file.safe_prepare_path(
                    &self.outpath,
                    &mut outpath,
                    None::<&(_, fn(&Path) -> bool)>,
                )?;
                self.resolved_paths
                    .insert(file.name_raw().into(), outpath.clone());

                if file.is_symlink() {
                    let declared_len = file.size();
                    if declared_len > MAX_SYMLINK_TARGET_LEN {
                        return Err(invalid!(
                            "symlink target declares {} bytes, more than the {} byte limit",
                            declared_len,
                            MAX_SYMLINK_TARGET_LEN
                        ));
                    }

                    let mut target = Vec::with_capacity(declared_len as usize);
                    file.read_to_end(&mut target)?;
                    match extract_options.symlink_action {
                        SymlinkExtractAction::ExtractAsFile => {
                            make_symlink_as_file(&outpath, &target)?;
                        }
                        SymlinkExtractAction::ExtractAsFileUtf8 => {
                            make_symlink_as_file_utf8(&outpath, &target)?;
                        }
                        _ => {
                            make_symlink(
                                &self.outpath,
                                &outpath,
                                &target,
                                &self.resolved_paths,
                                extract_options.symlink_action,
                            )?;
                        }
                    }
                    return Ok(());
                }
                if file.is_dir() {
                    std::fs::create_dir_all(&outpath)?;
                } else {
                    let mut outfile = std::fs::File::create(&outpath)?;
                    io::copy(file, &mut outfile)?;
                }

                Ok(())
            }

            fn visit_additional_metadata(&mut self, metadata: &ZipFileEntry<'_>) -> ZipResult<()> {
                if let (Some(outpath), Some(mode)) = (
                    self.resolved_paths.get(metadata.name_raw()),
                    metadata.unix_mode(),
                ) {
                    self.file_permissions.insert(outpath.clone(), mode);
                }
                Ok(())
            }
        }
        /// Moves the entries `visit` wrote under `staging` to the same place under `dest`,
        /// resolving each path again as `visit_file` does.
        fn move_staged_entries(
            staging: &mut PathBuf,
            dest: &Path,
            extractor: &mut Extractor,
        ) -> ZipResult<()> {
            let written_to = staging.clone();
            let has_entry_named_like = |dir: &Path| {
                dir.file_name()
                    .is_some_and(|name| dir.join(name).symlink_metadata().is_ok())
            };
            while has_entry_named_like(staging) {
                let renamed = staging_path(dest)?;
                std::fs::rename(&*staging, &renamed)?;
                *staging = renamed;
            }

            for staged in extractor.resolved_paths.values_mut() {
                let relative = staged
                    .strip_prefix(&written_to)
                    .map_err(|_| invalid!("Entry was not extracted to the staging directory"))?;
                let from = staging.join(relative);
                let components = relative.iter().map(|c| c.to_os_string());
                let outpath = crate::path::resolve_enclosed(dest, dest, components, true)?;
                if outpath.starts_with(&*staging) {
                    return Err(invalid!("Entry would be written to the staging directory"));
                }
                if std::fs::symlink_metadata(&from)?.is_dir() {
                    std::fs::create_dir_all(&outpath)?;
                } else if let Err(e) = std::fs::rename(&from, &outpath) {
                    if e.kind() != io::ErrorKind::CrossesDevices {
                        return Err(e.into());
                    }
                    std::fs::copy(&from, &outpath)?;
                }
                if let Some(mode) = extractor.file_permissions.remove(&*staged) {
                    extractor.file_permissions.insert(outpath.clone(), mode);
                }
                *staged = outpath;
            }
            Ok(())
        }

        std::fs::create_dir_all(&directory)?;
        let directory = directory.as_ref().canonicalize()?;

        let staging = if self.enforce_header_consistency {
            Some(create_staging_dir(&directory)?)
        } else {
            None
        };

        let mut extractor = Extractor {
            outpath: staging.clone().unwrap_or_else(|| directory.clone()),
            resolved_paths: IndexMap::new(),
            file_permissions: Default::default(),
        };
        let visited = self.visit(&mut extractor);
        if let Some(mut staging) = staging {
            let moved = visited
                .and_then(|()| move_staged_entries(&mut staging, &directory, &mut extractor));
            let removed = std::fs::remove_dir_all(&staging);
            moved?;
            removed?;
        } else {
            visited?;
        }

        for (path, mode) in extractor.file_permissions.into_iter().rev() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                set_permissions_no_follow_symlink(&path, std::fs::Permissions::from_mode(mode))?;
            }
            #[cfg(not(unix))]
            if mode & 0o222 == 0 {
                let metadata = std::fs::symlink_metadata(&path)?;
                if !metadata.is_symlink() {
                    let mut permissions = metadata.permissions();
                    permissions.set_readonly(true);
                    std::fs::set_permissions(&path, permissions)?;
                }
            }
        }
        Ok(())
    }
}

/// A path in `dest` with a random name.
fn staging_path(dest: &Path) -> io::Result<PathBuf> {
    let n = getrandom::u64().map_err(io::Error::other)?;
    Ok(dest.join(format!(".zip-extract-{n:016x}")))
}

/// Creates a new, empty directory with a random name in `dest`.
fn create_staging_dir(dest: &Path) -> io::Result<PathBuf> {
    loop {
        let staging = staging_path(dest)?;
        match std::fs::create_dir(&staging) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            result => return result.map(|()| staging),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Symlinks being extracted shouldn't be followed out of the destination directory.
    /// Cannot use fs with miri CI
    #[cfg(not(miri))]
    #[test]
    fn test_cannot_symlink_outside_destination() -> ZipResult<()> {
        use crate::ZipWriter;
        use crate::write::SimpleFileOptions;
        use std::fs::create_dir;
        use tempfile::TempDir;

        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.add_symlink("symlink/", "../dest-sibling/", SimpleFileOptions::default())?;
        writer.start_file("symlink/dest-file", SimpleFileOptions::default())?;
        let reader = ZipStreamReader::new(writer.finish()?);
        let dest_parent = TempDir::with_prefix("stream__cannot_symlink_outside_destination")?;
        let dest_sibling = dest_parent.path().join("dest-sibling");
        create_dir(&dest_sibling)?;
        let dest = dest_parent.path().join("dest");
        create_dir(&dest)?;
        assert!(reader.extract(dest).is_err());
        assert!(!dest_sibling.join("dest-file").exists());
        Ok(())
    }

    /// Cannot use fs with miri CI
    #[cfg(not(miri))]
    #[test]
    fn test_can_create_destination() -> ZipResult<()> {
        use tempfile::TempDir;

        let v = include_bytes!("../../tests/data/mimetype.zip");
        let reader = ZipStreamReader::new(v.as_ref());
        let dest = TempDir::with_prefix("stream_test_can_create_destination").unwrap();
        reader.extract(&dest)?;
        assert!(dest.path().join("mimetype").exists());
        Ok(())
    }
}
