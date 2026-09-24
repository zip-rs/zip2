//! Extraction

use crate::read::MAX_SYMLINK_TARGET_LEN;
use crate::result::{ZipResult, invalid};
use crate::{ZipArchive, result::ZipError};
use std::ffi::OsStr;
use std::io::{self, Read, Seek};
use std::path::Path;

use indexmap::IndexMap;

pub(crate) fn make_writable_dir_all<T: AsRef<Path>>(outpath: T) -> Result<(), ZipError> {
    use std::fs;
    fs::create_dir_all(outpath.as_ref())?;
    #[cfg(unix)]
    {
        // Dirs must be writable until all normal files are extracted
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            outpath.as_ref(),
            std::fs::Permissions::from_mode(
                0o700 | std::fs::metadata(outpath.as_ref())?.permissions().mode(),
            ),
        )?;
    }
    Ok(())
}

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
) -> ZipResult<()> {
    let Ok(target_str) = std::str::from_utf8(target) else {
        return Err(invalid!("Invalid UTF-8 as symlink target"));
    };

    // Check where the symlink would resolve to before creating it. A relative target
    // is resolved from the parent of the symlink itself, exactly as the OS would.
    let target_path = Path::new(target_str);
    let (start, rest) = if target_path.is_absolute() {
        let Some(rest) = crate::path::strip_base_prefix(base, target_path) else {
            return Err(invalid!("Symlink target escapes the destination directory"));
        };
        (base.to_path_buf(), rest.to_path_buf())
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
        (parent.to_path_buf(), target_path.to_path_buf())
    };

    crate::path::resolve_enclosed(
        base,
        &start,
        rest.components().map(|c| c.as_os_str().to_os_string()),
        false, // only checking here, so nothing is created
    )?;

    make_symlink_impl(outpath, target_str, existing_files)
}

#[cfg(not(any(windows, unix)))]
pub(crate) fn make_symlink<T>(
    // No symlink is created on these targets, so there is nothing to contain.
    _base: &Path,
    outpath: &Path,
    target: &[u8],
    #[cfg_attr(not(any(windows, unix)), allow(unused))] existing_files: &IndexMap<Box<[u8]>, T>,
) -> ZipResult<()> {
    let Ok(_) = std::str::from_utf8(target) else {
        return Err(invalid!("Invalid UTF-8 as symlink target"));
    };
    use std::fs::File;
    let output = File::create(outpath);
    output?.write_all(target)?;
    Ok(())
}

/// Store all entries which specify a numeric "mode" which is familiar to POSIX operating systems.
#[cfg(unix)]
#[derive(Default, Debug)]
struct UnixFileModes {
    map: std::collections::BTreeMap<std::path::PathBuf, u32>,
}

#[cfg(unix)]
impl UnixFileModes {
    #[cfg_attr(not(debug_assertions), allow(unused))]
    pub fn add_mode(&mut self, path: std::path::PathBuf, mode: u32) {
        // We don't print a warning or consider it remotely out of the ordinary to receive two
        // separate modes for the same path: just take the later one.
        let old_entry = self.map.insert(path, mode);
        debug_assert_eq!(old_entry, None);
    }

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
        self.extract_internal(directory, None::<fn(&Path) -> bool>)
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
        self.extract_internal(directory, Some(root_dir_filter))
    }

    fn extract_internal<P: AsRef<Path>>(
        &mut self,
        directory: P,
        root_dir_filter: Option<impl RootDirFilter>,
    ) -> ZipResult<()> {
        use std::fs;

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

        #[cfg(unix)]
        let mut files_by_unix_mode = UnixFileModes::default();

        for i in 0..self.len() {
            let mut file = self.by_index(i)?;

            let mut outpath = directory.clone();
            /* TODO: the control flow of this method call and subsequent expectations about the
             *       values in this loop is extremely difficult to follow. It also appears to
             *       perform a nested loop upon extracting every single file entry? Why does it
             *       accept two arguments that point to the same directory path, one mutable? */
            file.safe_prepare_path(directory.as_ref(), &mut outpath, root_dir.as_ref())?;

            #[cfg(any(unix, windows))]
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
                make_symlink(directory.as_ref(), &outpath, &target, &self.shared.files)?;
                continue;
            } else if file.is_dir() {
                make_writable_dir_all(&outpath)?;
                continue;
            }
            let mut outfile = fs::File::create(&outpath)?;
            io::copy(&mut file, &mut outfile)?;

            // Check for real permissions, which we'll set in a second pass.
            #[cfg(unix)]
            if let Some(mode) = file.unix_mode() {
                files_by_unix_mode.add_mode(outpath, mode);
            }

            // Set original timestamp.
            #[cfg(feature = "chrono")]
            if let Some(last_modified) = file.last_modified()
                && let Some(t) = last_modified.datetime_to_systemtime()
            {
                outfile.set_modified(t)?;
            }
        }

        // Ensure we update children's permissions before making a parent unwritable.
        #[cfg(unix)]
        for (path, perms) in files_by_unix_mode.all_perms_with_children_first() {
            std::fs::set_permissions(path, perms)?;
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
