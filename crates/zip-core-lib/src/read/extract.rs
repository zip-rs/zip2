//! Shared extraction option and path-filter types.

use std::ffi::OsStr;
use std::path::Path;
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
    pub symlink_action: SymlinkExtractAction,
    pub size_limit: Option<u64>,
    pub enforce_header_consistency: bool,
}

impl ExtractOptions {
    /// Returns the action used when an archive entry is a symbolic link.
    #[must_use]
    pub fn symlink_action_value(&self) -> SymlinkExtractAction {
        self.symlink_action
    }

    /// Returns the declared-size limit, if one was set.
    #[must_use]
    pub fn size_limit_value(&self) -> Option<u64> {
        self.size_limit
    }

    /// Returns whether local and central headers are checked for consistency.
    #[must_use]
    pub fn header_consistency_enforced(&self) -> bool {
        self.enforce_header_consistency
    }

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

    /// Enforce consistency between local file headers and central directory headers during extraction.
    #[must_use]
    pub fn enforce_header_consistency(self, enforce: bool) -> Self {
        Self {
            enforce_header_consistency: enforce,
            ..self
        }
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
