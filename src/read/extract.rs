//! Extraction

use crate::result::{ZipResult, invalid};
use crate::{ZipArchive, result::ZipError};
use std::ffi::OsStr;
use std::io::{Read, Seek, Write};
use std::path::Path;

use indexmap::IndexMap;

#[cfg(feature = "rayon")]
use rayon::prelude::*;

/// Upper bound on the uncompressed size of a symlink entry.
///
/// A symlink target is a filesystem path, so its length is bounded by the
/// platform (`PATH_MAX` is 4096 on Linux) no matter what the archive says. The
/// uncompressed size stored in the central directory is attacker-controlled,
/// so entries above this bound are rejected rather than pre-allocated.
pub(crate) const MAX_SYMLINK_TARGET_LEN: u64 = 4096;

/// Buffer capacity for file extraction streams (64 KiB), reducing syscall overhead
/// and optimizing page cache utilization during archive extraction.
pub(crate) const EXTRACT_BUFFER_SIZE: usize = 64 * 1024;

/// Maximum extent pre-allocation ceiling (64 MiB).
///
/// Pre-allocating disk space via `File::set_len` before streaming uncompressed bytes
/// reduces filesystem fragmentation and optimizes NTFS/ext4 block allocation.
/// Capped at 64 MiB to prevent denial-of-service / zero-fill amplification attacks
/// where an archive header claims an exorbitantly large uncompressed size.
pub(crate) const MAX_PREALLOCATE_LEN: u64 = 64 * 1024 * 1024;

/// Copies decompressed data from `reader` into `writer` using `buf` as a reusable transfer buffer.
pub(crate) fn copy_into<R: Read + ?Sized, W: Write + ?Sized>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut [u8],
) -> std::io::Result<u64> {
    let mut written = 0u64;
    loop {
        let n = reader.read(buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        written = written.saturating_add(n as u64);
    }
    Ok(written)
}

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
    pub(crate) enforce_header_consistency: bool,
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

    /// Enforce consistency between local file headers and central directory headers during extraction.
    #[must_use]
    pub fn enforce_header_consistency(self, enforce: bool) -> Self {
        Self {
            enforce_header_consistency: enforce,
            ..self
        }
    }
}

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
                        rest.components().map(|c| c.as_os_str()),
                        false,
                    )?;
                }
            }
        }
    } else {
        crate::path::resolve_enclosed(
            base,
            start,
            rest.components().map(|c| c.as_os_str()),
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
    make_symlink_as_file_utf8(outpath, target, None)?;
    Ok(())
}

pub(crate) fn make_symlink_as_file_utf8(
    outpath: &Path,
    target: &[u8],
    unix_mode: Option<u32>,
) -> ZipResult<()> {
    let Ok(_) = std::str::from_utf8(target) else {
        return Err(invalid!("Invalid UTF-8 as symlink target"));
    };
    make_symlink_as_file(outpath, target, unix_mode)?;
    Ok(())
}

/// Create an output file for extraction securely.
///
/// If `outpath` already exists as a symbolic link, it is removed before creating the regular
/// file so that extraction will not follow the link and overwrite a target file outside the
/// destination directory. After creation, the file is verified to ensure it was not swapped
/// for a symbolic link.
#[cfg_attr(not(unix), allow(unused_variables))]
pub(crate) fn create_outfile(
    outpath: &Path,
    unix_mode: Option<u32>,
) -> std::io::Result<std::fs::File> {
    if let Ok(meta) = std::fs::symlink_metadata(outpath)
        && meta.file_type().is_symlink()
    {
        std::fs::remove_file(outpath)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(true); // Default for `fs::File::create`
    #[cfg(unix)]
    if let Some(mode) = unix_mode {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(
            windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT
                | windows_sys::Win32::Storage::FileSystem::FILE_FLAG_SEQUENTIAL_SCAN,
        );
    }
    let file = options.open(outpath)?;

    if file.metadata()?.file_type().is_symlink()
        || std::fs::symlink_metadata(outpath).is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Extracted file cannot be written through a symbolic link",
        ));
    }
    Ok(file)
}

pub(crate) fn make_symlink_as_file(
    outpath: &Path,
    target: &[u8],
    unix_mode: Option<u32>,
) -> std::io::Result<()> {
    use std::io::Write;
    let mut output = create_outfile(outpath, unix_mode)?;
    output.write_all(target)?;
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
    /// Extraction is also not atomic in term of permissions. Permissions are applied after the
    /// extraction (because some folder have stricter permissions when we cannot access it). That
    /// means, for a moment, a file could be created with default permissions (and then changed
    /// to the correct one). On Unix this only applies to directories and symlinks.
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

    fn extract_internal<P: AsRef<Path>>(
        &mut self,
        directory: P,
        root_dir_filter: Option<impl RootDirFilter>,
        extract_options: ExtractOptions,
    ) -> ZipResult<()> {
        use std::fs;

        if extract_options.enforce_header_consistency {
            self.validate_header_consistency()?;
        }

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
        let mut extract_buf = vec![0u8; EXTRACT_BUFFER_SIZE];
        let mut created_dirs = std::collections::HashSet::new();

        for i in 0..self.len() {
            let read_options = crate::read::ZipReadOptions::new()
                .enforce_header_consistency(extract_options.enforce_header_consistency);
            let mut file = self.by_index_with_options(i, read_options)?;

            let outpath = file.safe_prepare_path(directory.as_ref(), root_dir.as_ref())?;

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
                let unix_mode = file.unix_mode();
                drop(file);
                match extract_options.symlink_action {
                    SymlinkExtractAction::NoExtract => {
                        // do nothing
                    }
                    SymlinkExtractAction::ExtractAsFile => {
                        make_symlink_as_file(&outpath, &target, unix_mode)?;
                    }
                    SymlinkExtractAction::ExtractAsFileUtf8 => {
                        make_symlink_as_file_utf8(&outpath, &target, unix_mode)?;
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
                if created_dirs.insert(outpath.clone()) {
                    make_writable_dir_all(&outpath)?;
                }
                // Check for real permissions, which we'll set in a second pass.
                if let Some(mode) = file.unix_mode() {
                    files_by_unix_mode.add_mode(outpath, mode);
                }
            } else {
                let mut outfile = create_outfile(&outpath, file.unix_mode())?;
                let uncompressed_len = file.size();
                let mut preallocated_len = 0u64;
                if uncompressed_len > 0 && !file.flags().is_using_data_descriptor() {
                    let alloc_len = uncompressed_len.min(MAX_PREALLOCATE_LEN);
                    if outfile.set_len(alloc_len).is_ok() {
                        preallocated_len = alloc_len;
                    }
                }
                let copy_res = copy_into(&mut file, &mut outfile, &mut extract_buf);
                if preallocated_len > 0 {
                    let actual_len = match &copy_res {
                        Ok(extracted) => *extracted,
                        Err(_) => outfile.stream_position().unwrap_or(0),
                    };
                    if actual_len < preallocated_len {
                        let _ = outfile.set_len(actual_len);
                    }
                }
                copy_res?;
                #[cfg(unix)]
                if let Some(mode) = file.unix_mode() {
                    // OpenOptions::mode only applies when the file is created. Record modes
                    // for the final pass too, so overwriting an existing path updates them.
                    files_by_unix_mode.add_mode(outpath, mode);
                }
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

#[cfg(feature = "rayon")]
impl<R: Read + Seek + Send> ZipArchive<R> {
    /// Extracts archive contents in parallel across available CPU cores using a reader factory.
    ///
    /// The `reader_factory` closure is called to produce an independent reader instance
    /// for each worker thread, ensuring independent file offsets and preventing concurrent seek collisions.
    ///
    /// # Common `reader_factory` Implementations
    ///
    /// Parallel extraction requires that each worker thread possesses an independent seek
    /// position into the archive stream. Common implementations include:
    ///
    /// - **In-memory slice (`Cursor<&[u8]>`)**:
    ///   Allows all worker threads to safely share an immutable buffer in memory without reallocation:
    ///   ```no_run
    ///   # use zip::ZipArchive;
    ///   # use std::io::Cursor;
    ///   # fn example(slice: &[u8]) -> zip::result::ZipResult<()> {
    ///   # let mut archive = ZipArchive::new(Cursor::new(slice))?;
    ///   archive.par_extract_with_reader_factory("destination", || Ok(Cursor::new(&slice)))?;
    ///   # Ok(())
    ///   # }
    ///   ```
    ///
    /// - **Cloned file descriptor (`File::try_clone`)**:
    ///   Duplicates the file handle so that each worker thread maintains an independent file pointer:
    ///   ```no_run
    ///   # use zip::ZipArchive;
    ///   # use std::fs::File;
    ///   # fn example(file: &File) -> zip::result::ZipResult<()> {
    ///   # let mut archive = ZipArchive::new(file.try_clone().unwrap())?;
    ///   archive.par_extract_with_reader_factory("destination", || file.try_clone())?;
    ///   # Ok(())
    ///   # }
    ///   ```
    ///
    /// - **Positional / Random-access cursor (`positioned_io::Cursor`)**:
    ///   Enables positional reads without duplicating kernel file handles:
    ///   ```ignore
    ///   archive.par_extract_with_reader_factory("destination", || {
    ///       Ok(positioned_io::Cursor::new(random_access_file))
    ///   })?;
    ///   ```
    pub fn par_extract_with_reader_factory<P, F>(
        &mut self,
        directory: P,
        reader_factory: F,
    ) -> ZipResult<()>
    where
        P: AsRef<Path>,
        F: Fn() -> std::io::Result<R> + Sync + Send,
    {
        self.par_extract_with_reader_factory_and_options(
            directory,
            reader_factory,
            ExtractOptions::default(),
            None::<fn(&Path) -> bool>,
        )
    }

    /// Extracts archive contents in parallel using a reader factory, custom [`ExtractOptions`],
    /// and an optional [`RootDirFilter`].
    ///
    /// See [`ZipArchive::par_extract_with_reader_factory`] for common `reader_factory` implementations.
    pub fn par_extract_with_reader_factory_and_options<P, F, Filter>(
        &mut self,
        directory: P,
        reader_factory: F,
        extract_options: ExtractOptions,
        root_dir_filter: Option<Filter>,
    ) -> ZipResult<()>
    where
        P: AsRef<Path>,
        F: Fn() -> std::io::Result<R> + Sync + Send,
        Filter: RootDirFilter,
    {
        self.par_extract_internal(
            directory.as_ref(),
            &reader_factory,
            extract_options,
            root_dir_filter,
        )
    }

    fn par_extract_internal<F, Filter>(
        &mut self,
        directory: &Path,
        reader_factory: &F,
        extract_options: ExtractOptions,
        root_dir_filter: Option<Filter>,
    ) -> ZipResult<()>
    where
        F: Fn() -> std::io::Result<R> + Sync + Send,
        Filter: RootDirFilter,
    {
        if let Some(limit) = extract_options.size_limit {
            match self.decompressed_size() {
                Some(size) if size > u128::from(limit) => {
                    return Err(ZipError::DecompressedSizeLimitExceeded { size, limit });
                }
                None => return Err(ZipError::DecompressedSizeUnknown),
                Some(_) => {}
            }
        }

        std::fs::create_dir_all(directory)?;
        let directory = directory.canonicalize()?;

        let root_dir = root_dir_filter
            .and_then(|filter| {
                self.root_dir(&filter)
                    .transpose()
                    .map(|root_dir| root_dir.map(|root_dir| (root_dir, filter)))
            })
            .transpose()?;

        let root_dir = root_dir
            .as_ref()
            .map(|(root_dir, filter)| {
                crate::path::simplified_components(root_dir)
                    .ok_or_else(|| {
                        debug_assert!(false, "Invalid root dir path");
                        invalid!("Invalid root dir path")
                    })
                    .map(|root_dir| (root_dir, filter))
            })
            .transpose()?;

        let mut files_by_unix_mode = UnixFileModes::default();
        let mut created_dirs = std::collections::HashSet::new();
        let mut regular_files: indexmap::IndexMap<std::path::PathBuf, usize> =
            indexmap::IndexMap::new();
        let mut total_extracted = 0u64;

        for i in 0..self.len() {
            let read_options = crate::read::ZipReadOptions::new()
                .enforce_header_consistency(extract_options.enforce_header_consistency);
            let mut file = self.by_index_with_options(i, read_options)?;

            let outpath = file.safe_prepare_path(directory.as_ref(), root_dir.as_ref())?;

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
                let symlink_bytes = target.len() as u64;
                let new_total = total_extracted.checked_add(symlink_bytes).ok_or_else(|| {
                    ZipError::DecompressedSizeLimitExceeded {
                        size: u128::MAX,
                        limit: extract_options.size_limit.unwrap_or(u64::MAX),
                    }
                })?;
                if let Some(limit) = extract_options.size_limit
                    && new_total > limit
                {
                    return Err(ZipError::DecompressedSizeLimitExceeded {
                        size: u128::from(new_total),
                        limit,
                    });
                }
                total_extracted = new_total;
                let unix_mode = file.unix_mode();
                drop(file);
                match extract_options.symlink_action {
                    SymlinkExtractAction::NoExtract => {}
                    SymlinkExtractAction::ExtractAsFile => {
                        make_symlink_as_file(&outpath, &target, unix_mode)?;
                    }
                    SymlinkExtractAction::ExtractAsFileUtf8 => {
                        make_symlink_as_file_utf8(&outpath, &target, unix_mode)?;
                    }
                    #[cfg(any(unix, windows))]
                    SymlinkExtractAction::ExtractRecursiveInFolder
                    | SymlinkExtractAction::ExtractInFolder => {
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
                if created_dirs.insert(outpath.clone()) {
                    make_writable_dir_all(&outpath)?;
                }
                if let Some(mode) = file.unix_mode() {
                    files_by_unix_mode.add_mode(outpath, mode);
                }
            } else {
                regular_files.insert(outpath, i);
            }
        }

        let metadata = self.metadata();
        let size_limit = extract_options.size_limit;
        let enforce_header = extract_options.enforce_header_consistency;
        let total_extracted_atomic = std::sync::atomic::AtomicU64::new(total_extracted);

        let regular_files_vec: Vec<(std::path::PathBuf, usize)> =
            regular_files.into_iter().collect();

        let regular_file_results: ZipResult<Vec<(std::path::PathBuf, Option<u32>)>> =
            regular_files_vec
                .into_par_iter()
                .map_init(
                    || -> std::io::Result<(ZipArchive<R>, Vec<u8>)> {
                        let reader = reader_factory()?;
                        let archive = unsafe {
                            ZipArchive::unsafe_new_with_metadata(reader, metadata.clone())
                        };
                        let buf = vec![0u8; EXTRACT_BUFFER_SIZE];
                        Ok((archive, buf))
                    },
                    |state, (outpath, idx)| -> ZipResult<(std::path::PathBuf, Option<u32>)> {
                        let (archive, buf) = match state {
                            Ok(s) => s,
                            Err(e) => {
                                return Err(ZipError::Io(std::io::Error::new(
                                    e.kind(),
                                    e.to_string(),
                                )));
                            }
                        };

                        let read_options = crate::read::ZipReadOptions::new()
                            .enforce_header_consistency(enforce_header);
                        let mut file = archive.by_index_with_options(idx, read_options)?;

                        let mut outfile = create_outfile(&outpath, file.unix_mode())?;
                        let uncompressed_len = file.size();
                        let mut preallocated_len = 0u64;

                        if uncompressed_len > 0 && !file.flags().is_using_data_descriptor() {
                            let alloc_len = uncompressed_len.min(MAX_PREALLOCATE_LEN);
                            let can_preallocate = match size_limit {
                                Some(limit) => {
                                    let cur = total_extracted_atomic
                                        .load(std::sync::atomic::Ordering::Relaxed);
                                    cur.saturating_add(alloc_len) <= limit
                                }
                                None => true,
                            };
                            if can_preallocate && outfile.set_len(alloc_len).is_ok() {
                                preallocated_len = alloc_len;
                            }
                        }

                        let mut file_extracted = 0u64;
                        let mut copy_error = None;
                        loop {
                            let n = match file.read(buf) {
                                Ok(0) => break,
                                Ok(n) => n,
                                Err(e) => {
                                    copy_error = Some(ZipError::Io(e));
                                    break;
                                }
                            };
                            let n_u64 = n as u64;
                            let old_total = total_extracted_atomic
                                .fetch_add(n_u64, std::sync::atomic::Ordering::Relaxed);
                            let new_total = match old_total.checked_add(n_u64) {
                                Some(total) => total,
                                None => {
                                    copy_error = Some(ZipError::DecompressedSizeLimitExceeded {
                                        size: u128::MAX,
                                        limit: size_limit.unwrap_or(u64::MAX),
                                    });
                                    break;
                                }
                            };
                            if let Some(limit) = size_limit
                                && new_total > limit
                            {
                                copy_error = Some(ZipError::DecompressedSizeLimitExceeded {
                                    size: u128::from(new_total),
                                    limit,
                                });
                                break;
                            }
                            if let Err(e) = outfile.write_all(&buf[..n]) {
                                copy_error = Some(ZipError::Io(e));
                                break;
                            }
                            file_extracted = file_extracted.saturating_add(n_u64);
                        }

                        if preallocated_len > 0 {
                            let actual_len = if copy_error.is_none() {
                                file_extracted
                            } else {
                                outfile.stream_position().unwrap_or(0)
                            };
                            if actual_len < preallocated_len {
                                let _ = outfile.set_len(actual_len);
                            }
                        }

                        if let Some(err) = copy_error {
                            return Err(err);
                        }

                        #[cfg(feature = "chrono")]
                        if let Some(last_modified) = file.last_modified()
                            && let Some(t) = last_modified.datetime_to_systemtime()
                        {
                            let _ = outfile.set_modified(t);
                        }
                        #[cfg(not(unix))]
                        if file.is_read_only() {
                            let mut permissions = outfile.metadata()?.permissions();
                            permissions.set_readonly(true);
                            outfile.set_permissions(permissions)?;
                        }

                        Ok((outpath, file.unix_mode()))
                    },
                )
                .collect();

        #[cfg_attr(not(unix), allow(unused_variables))]
        let extracted_files = regular_file_results?;

        #[cfg(unix)]
        for (path, mode) in extracted_files {
            if let Some(m) = mode {
                files_by_unix_mode.add_mode(path, m);
            }
        }

        #[cfg(unix)]
        for (path, perms) in files_by_unix_mode.all_perms_with_children_first() {
            set_permissions_no_follow_symlink(&path, perms)?;
        }
        #[cfg(not(unix))]
        for path in files_by_unix_mode.all_read_only_with_children_first() {
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                let mut permissions = metadata.permissions();
                permissions.set_readonly(true);
                std::fs::set_permissions(path, permissions)?;
            }
        }

        Ok(())
    }
}

#[cfg(feature = "rayon")]
impl ZipArchive<std::fs::File> {
    /// Extracts a filesystem-backed archive in parallel across available CPU cores.
    ///
    /// Requires the path to the original archive file (`archive_path`) so that each
    /// worker thread can open its own independent kernel file handle via `File::open`.
    pub fn par_extract_from_path<P: AsRef<Path>, Q: AsRef<Path>>(
        &mut self,
        archive_path: P,
        destination: Q,
    ) -> ZipResult<()> {
        let path_buf = archive_path.as_ref().to_path_buf();
        self.par_extract_with_reader_factory(destination, move || std::fs::File::open(&path_buf))
    }

    /// Extracts a filesystem-backed archive in parallel with custom [`ExtractOptions`].
    pub fn par_extract_from_path_with_options<P: AsRef<Path>, Q: AsRef<Path>>(
        &mut self,
        archive_path: P,
        destination: Q,
        options: ExtractOptions,
    ) -> ZipResult<()> {
        let path_buf = archive_path.as_ref().to_path_buf();
        self.par_extract_with_reader_factory_and_options(
            destination,
            move || std::fs::File::open(&path_buf),
            options,
            None::<fn(&Path) -> bool>,
        )
    }
}

#[cfg(feature = "rayon")]
impl<T: AsRef<[u8]> + Sync + Send + Clone> ZipArchive<std::io::Cursor<T>> {
    /// Extracts an in-memory archive in parallel across available CPU cores.
    pub fn par_extract<P: AsRef<Path>>(&mut self, directory: P) -> ZipResult<()> {
        let cursor_data = self.reader.get_ref().clone();
        self.par_extract_with_reader_factory(directory, move || {
            Ok(std::io::Cursor::new(cursor_data.clone()))
        })
    }

    /// Extracts an in-memory archive in parallel with custom [`ExtractOptions`].
    pub fn par_extract_with_options<P: AsRef<Path>>(
        &mut self,
        directory: P,
        options: ExtractOptions,
    ) -> ZipResult<()> {
        let cursor_data = self.reader.get_ref().clone();
        self.par_extract_with_reader_factory_and_options(
            directory,
            move || Ok(std::io::Cursor::new(cursor_data.clone())),
            options,
            None::<fn(&Path) -> bool>,
        )
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
