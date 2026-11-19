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

/// Buffer capacity for file extraction streams, reducing syscall overhead
/// and optimizing page cache utilization during archive extraction.
pub(crate) const EXTRACT_BUFFER_SIZE: usize = 256 * 1024;

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
///
/// `F` is the type of the root-directory filter set with [`ExtractOptions::with_root_dir_filter`].
#[non_exhaustive]
pub struct ExtractOptions<F = fn(&Path) -> bool> {
    pub(crate) symlink_action: SymlinkExtractAction,
    pub(crate) size_limit: Option<u64>,
    pub(crate) enforce_header_consistency: bool,
    pub(crate) root_dir_filter: Option<F>,
}

impl<F> core::fmt::Debug for ExtractOptions<F> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ExtractOptions")
            .field("symlink_action", &self.symlink_action)
            .field("size_limit", &self.size_limit)
            .field(
                "enforce_header_consistency",
                &self.enforce_header_consistency,
            )
            .field(
                "root_dir_filter",
                &self.root_dir_filter.as_ref().map(|_| "<filter>"),
            )
            .finish()
    }
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            symlink_action: SymlinkExtractAction::default(),
            size_limit: None,
            enforce_header_consistency: false,
            root_dir_filter: None,
        }
    }
}

impl<F> ExtractOptions<F> {
    /// Unwrap a single top-level "root" directory during extraction, using `filter`
    /// to decide which entries are considered when detecting it.
    ///
    /// Entries for which `filter` returns `false` are ignored while searching for the
    /// root directory, but they are **still extracted**; this does not exclude entries
    /// from extraction. If no root directory is found, the archive is extracted as by
    /// [`ZipArchive::extract`]. See [`ZipArchive::root_dir`] for how the root directory
    /// is detected.
    ///
    /// For a sensible default, use [`root_dir_common_filter`]. For a custom filter,
    /// see [`RootDirFilter`].
    ///
    /// # Example
    ///
    /// ```
    /// use zip::read::{ExtractOptions, root_dir_common_filter};
    ///
    /// let options = ExtractOptions::default()
    ///     .with_root_dir_filter(root_dir_common_filter)
    ///     .with_size_limit(1 << 30);
    /// # let _ = format!("{options:?}");
    /// ```
    #[must_use]
    pub fn with_root_dir_filter<G: RootDirFilter>(self, filter: G) -> ExtractOptions<G> {
        ExtractOptions {
            symlink_action: self.symlink_action,
            size_limit: self.size_limit,
            enforce_header_consistency: self.enforce_header_consistency,
            root_dir_filter: Some(filter),
        }
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

#[cfg(unix)]
fn cstr(c: &OsStr) -> std::io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(c.as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "NUL byte in path component",
        )
    })
}

#[cfg(unix)]
fn is_nofollow_error(err: &std::io::Error) -> bool {
    #[cfg(target_os = "freebsd")]
    if err.raw_os_error() == Some(libc::EMLINK) {
        return true;
    }
    #[cfg(target_os = "netbsd")]
    if err.raw_os_error() == Some(libc::EFTYPE) {
        return true;
    }
    matches!(err.raw_os_error(), Some(libc::ELOOP))
}

/// Normalize `libc::stat::st_mode` to `libc::mode_t`.
///
/// On 32-bit Android targets (e.g. `armv7-linux-androideabi`, `i686-linux-android`),
/// `libc::stat::st_mode` is `u32` (`c_uint`) while `libc::mode_t` and the `S_IF*`
/// constants are `u16`.
#[cfg(unix)]
// `st.st_mode` already has type `libc::mode_t` on most Unix targets, so the cast is only narrowing on 32-bit Android.
#[allow(clippy::unnecessary_cast)]
const fn stat_mode(st: &libc::stat) -> libc::mode_t {
    st.st_mode as libc::mode_t
}

/// Descend from `dest` to `dir_path` component by component using `openat` with
/// `O_DIRECTORY | O_NOFOLLOW`, optionally creating missing components with `mkdirat`
/// and ensuring owner-rwx (`0o700`) permissions on each visited directory descriptor.
///
/// Because every hop is resolved relative to the parent directory's open file descriptor
/// with `O_NOFOLLOW`, a concurrent symlink swap on any intermediate directory component
/// is rejected atomically by the kernel at open time.
#[cfg(unix)]
fn open_dir_beneath(
    dest: &Path,
    dir_path: &Path,
    create_and_make_writable: bool,
) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::io::{Error, ErrorKind};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::path::Component;

    let rel = crate::path::strip_base_prefix(dest, dir_path).ok_or_else(|| {
        Error::new(
            ErrorKind::PermissionDenied,
            "Output path escapes the destination directory",
        )
    })?;

    // Open the destination root. `dest` itself may legitimately be or reside behind
    // a symlink chosen by the caller (e.g. /tmp on macOS), whereas no component
    // beneath `dest` may be a symlink.
    let mut dirfd: OwnedFd = std::fs::File::open(dest)?.into();

    for comp in rel.components() {
        let comp_os = match comp {
            Component::CurDir => continue,
            Component::Normal(s) => s,
            _ => {
                return Err(Error::new(
                    ErrorKind::PermissionDenied,
                    "Invalid component in extraction path",
                ));
            }
        };
        let name = cstr(comp_os)?;
        if create_and_make_writable {
            // Create directories owner-only (0o700) so other local users cannot list
            // or exploit intermediate directories while extraction is in progress.
            let r = unsafe { libc::mkdirat(dirfd.as_raw_fd(), name.as_ptr(), 0o700) };
            if r != 0 {
                let e = Error::last_os_error();
                if e.raw_os_error() != Some(libc::EEXIST) {
                    return Err(e);
                }
            }
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let open_dir_flags = libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let open_dir_flags =
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let child = unsafe { libc::openat(dirfd.as_raw_fd(), name.as_ptr(), open_dir_flags) };
        if child < 0 {
            let e = Error::last_os_error();
            return Err(
                if is_nofollow_error(&e) || e.raw_os_error() == Some(libc::ENOTDIR) {
                    Error::new(
                        ErrorKind::PermissionDenied,
                        "Extracted file parent directory cannot be a symbolic link",
                    )
                } else {
                    e
                },
            );
        }
        let child_fd = unsafe { OwnedFd::from_raw_fd(child) };
        if create_and_make_writable {
            let mut st: libc::stat = unsafe { core::mem::zeroed() };
            if unsafe { libc::fstat(child_fd.as_raw_fd(), &mut st) } != 0 {
                return Err(Error::last_os_error());
            }
            let current_mode = stat_mode(&st) & 0o7777;
            if current_mode & 0o700 != 0o700 {
                let desired = current_mode | 0o700;
                #[cfg(any(target_os = "linux", target_os = "android"))]
                {
                    // Linux `O_PATH` descriptors only require search (`+x`) permission on the
                    // directory, and `fchmodat` on `/proc/self/fd/<fd>` updates the pinned inode.
                    let proc_path = std::ffi::CString::new(format!(
                        "/proc/self/fd/{}",
                        child_fd.as_raw_fd()
                    ))
                    .map_err(|_| Error::new(ErrorKind::InvalidInput, "NUL byte in fd path"))?;
                    if unsafe { libc::fchmodat(libc::AT_FDCWD, proc_path.as_ptr(), desired, 0) }
                        != 0
                    {
                        let err = Error::last_os_error();
                        if matches!(err.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR)) {
                            // Minimal containers/chroots without `/proc`: reopen readable dirs
                            // pinned beneath `dirfd` with `O_NOFOLLOW` and `fchmod`.
                            let ro_fd = unsafe {
                                libc::openat(
                                    dirfd.as_raw_fd(),
                                    name.as_ptr(),
                                    libc::O_RDONLY
                                        | libc::O_DIRECTORY
                                        | libc::O_NOFOLLOW
                                        | libc::O_CLOEXEC,
                                )
                            };
                            if ro_fd < 0 {
                                return Err(err);
                            }
                            let ro_owned = unsafe { OwnedFd::from_raw_fd(ro_fd) };
                            if unsafe { libc::fchmod(ro_owned.as_raw_fd(), desired) } != 0 {
                                return Err(Error::last_os_error());
                            }
                        } else {
                            return Err(err);
                        }
                    }
                }
                #[cfg(not(any(target_os = "linux", target_os = "android")))]
                if unsafe { libc::fchmod(child_fd.as_raw_fd(), desired) } != 0 {
                    return Err(Error::last_os_error());
                }
            }
        }
        dirfd = child_fd;
    }

    Ok(dirfd)
}

/// Descend from `dest` to `dir_path` component by component on Windows, opening each
/// intermediate directory with `FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS`
/// and `share_mode(FILE_SHARE_READ)` (omitting both `FILE_SHARE_WRITE` and `FILE_SHARE_DELETE`).
///
/// Holding every ancestor directory handle open without `FILE_SHARE_WRITE` or
/// `FILE_SHARE_DELETE` prevents any concurrent process from deleting, renaming, or
/// converting an empty intermediate directory into a symbolic link or NTFS junction
/// (`FSCTL_SET_REPARSE_POINT` requires write access on the directory) while child
/// directories, files, or symlinks are being created.
#[cfg(windows)]
fn open_dir_beneath_windows(
    dest: &Path,
    dir_path: &Path,
    create_missing: bool,
) -> std::io::Result<Vec<std::fs::File>> {
    use std::io::{Error, ErrorKind};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Component;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
        GetFileInformationByHandle,
    };

    let rel = crate::path::strip_base_prefix(dest, dir_path).ok_or_else(|| {
        Error::new(
            ErrorKind::PermissionDenied,
            "Output path escapes the destination directory",
        )
    })?;

    let mut cur = dest.to_path_buf();
    let mut handles = Vec::new();

    for comp in rel.components() {
        let comp_os = match comp {
            Component::CurDir => continue,
            Component::Normal(s) => s,
            _ => {
                return Err(Error::new(
                    ErrorKind::PermissionDenied,
                    "Invalid component in extraction path",
                ));
            }
        };
        cur.push(comp_os);
        if create_missing {
            match std::fs::create_dir(&cur) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(e)
                    if e.kind() == ErrorKind::PermissionDenied
                        && std::fs::symlink_metadata(&cur).is_ok() => {}
                Err(e) => return Err(e),
            }
        }
        // Include `FILE_LIST_DIRECTORY` so Windows' `IoCheckShareAccess` actually
        // participates in share-mode enforcement (metadata-only `FILE_READ_ATTRIBUTES`
        // bypasses share modes). Omitting `FILE_SHARE_WRITE` and `FILE_SHARE_DELETE`
        // then blocks concurrent `FILE_WRITE_DATA` (junction reparse-point attachment)
        // and `DELETE`/rename while still permitting concurrent directory readers and
        // child file creation.
        let dir_handle = std::fs::OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&cur)?;
        let mut info = unsafe { core::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        if unsafe { GetFileInformationByHandle(dir_handle.as_raw_handle() as _, &mut info) } == 0 {
            return Err(Error::last_os_error());
        }
        if is_disallowed_reparse_handle(dir_handle.as_raw_handle(), info.dwFileAttributes)? {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "Extracted file parent directory cannot be a symbolic link or disallowed reparse point",
            ));
        }
        if (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) == 0 {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "Extracted file parent is not a directory",
            ));
        }
        handles.push(dir_handle);
    }

    Ok(handles)
}

#[cfg_attr(not(any(unix, windows)), allow(unused_variables))]
// `destination` is used on Unix and Windows to anchor ancestor directory traversal.
pub(crate) fn make_writable_dir_all<T: AsRef<Path>>(
    outpath: T,
    destination: Option<&Path>,
) -> Result<(), ZipError> {
    use std::fs;
    let path = outpath.as_ref();
    #[cfg(unix)]
    if let Some(dest) = destination {
        open_dir_beneath(dest, path, true)?;
        return Ok(());
    }
    #[cfg(windows)]
    if let Some(dest) = destination {
        open_dir_beneath_windows(dest, path, true)?;
        return Ok(());
    }
    if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(invalid!(
            "Destination directory component cannot be a symbolic link"
        ));
    }
    #[allow(clippy::disallowed_methods)]
    // Fallback when no destination root is supplied or on targets without dirfd/reparse-point APIs.
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        // Dirs must be writable until all normal files are extracted
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            return Err(invalid!(
                "Destination directory component cannot be a symbolic link"
            ));
        }
        let perms = fs::Permissions::from_mode(0o700 | meta.permissions().mode());
        set_permissions_no_follow_symlink(path, perms, None)?;
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn make_symlink_impl<T>(
    outpath: &Path,
    target_str: &str,
    _existing_files: &IndexMap<Box<[u8]>, T>,
    destination: Option<&Path>,
) -> ZipResult<()> {
    if let Some(dest) = destination
        && let (Some(parent), Some(file_name)) = (outpath.parent(), outpath.file_name())
    {
        use std::os::fd::AsRawFd;
        let dirfd = open_dir_beneath(dest, parent, true)?;
        let link_name = cstr(file_name)?;
        let target_cstr = cstr(OsStr::new(target_str))?;
        let r =
            unsafe { libc::symlinkat(target_cstr.as_ptr(), dirfd.as_raw_fd(), link_name.as_ptr()) };
        if r != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        return Ok(());
    }
    std::os::unix::fs::symlink(Path::new(&target_str), outpath)?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn make_symlink_impl<T>(
    outpath: &Path,
    target_str: &str,
    existing_files: &IndexMap<Box<[u8]>, T>,
    destination: Option<&Path>,
) -> ZipResult<()> {
    let _pinned_ancestors = if let Some(dest) = destination
        && let Some(parent) = outpath.parent()
    {
        Some(open_dir_beneath_windows(dest, parent, true)?)
    } else {
        None
    };
    // Windows NT kernel reparse-point resolution requires `\` separators rather than `/`.
    let normalized_target = target_str.replace('/', "\\");
    let target = Path::new(OsStr::new(&normalized_target));

    let mut archive_comps: Vec<String> = Vec::new();
    let target_is_relative = !Path::new(target_str).is_absolute()
        && !target_str.starts_with('/')
        && !target_str.starts_with('\\');
    if target_is_relative {
        if let Some(dest) = destination
            && let Some(rel_parent) = outpath
                .parent()
                .and_then(|p| crate::path::strip_base_prefix(dest, p))
        {
            for c in rel_parent.components() {
                if let std::path::Component::Normal(s) = c {
                    archive_comps.push(s.to_string_lossy().into_owned());
                }
            }
        }
        let mut underflow = false;
        for part in target_str.split(['/', '\\']) {
            match part {
                "" | "." => {}
                ".." => {
                    if archive_comps.pop().is_none() {
                        underflow = true;
                        break;
                    }
                }
                other => archive_comps.push(other.to_owned()),
            }
        }
        if underflow {
            archive_comps.clear();
        }
    }
    let archive_dir_prefix = if !archive_comps.is_empty() {
        Some(format!("{}/", archive_comps.join("/")))
    } else {
        None
    };
    let trimmed_target = target_str.trim_end_matches(['/', '\\']);
    let exact_dir_key = (!trimmed_target.is_empty()).then(|| format!("{trimmed_target}/"));
    let target_is_dir_from_archive = exact_dir_key
        .as_ref()
        .is_some_and(|k| existing_files.contains_key(k.as_bytes()))
        || archive_dir_prefix.as_ref().is_some_and(|prefix| {
            existing_files
                .keys()
                .any(|k| k.starts_with(prefix.as_bytes()))
        });

    let target_is_dir = if target_is_dir_from_archive {
        true
    } else if !normalized_target.starts_with("\\\\") {
        // Normalize lexical `..` segments relative to `outpath.parent()` because verbatim `\\?\`
        // paths do not collapse `..` inside Win32 `metadata`.
        let mut probe = if target.is_relative() {
            outpath.parent().map(Path::to_path_buf).unwrap_or_default()
        } else {
            std::path::PathBuf::new()
        };
        for c in target.components() {
            match c {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    let _ = probe.pop();
                }
                other => probe.push(other.as_os_str()),
            }
        }
        std::fs::metadata(&probe).is_ok_and(|meta| meta.is_dir())
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
            #[cfg(unix)]
            let _pin_link_parent =
                open_dir_beneath(base, outpath.parent().unwrap_or(base), false).ok();
            #[cfg(windows)]
            let _pin_link_parent =
                open_dir_beneath_windows(base, outpath.parent().unwrap_or(base), false).ok();

            // Try to canonicalize the resolved path so that symlinks in the filesystem
            // that point back inside `base` are followed.
            let abs_to_canonicalize = if target_path.is_absolute() {
                target_path
            } else {
                &start.join(target_path)
            };
            let canonical_in_base = match std::fs::canonicalize(abs_to_canonicalize) {
                Ok(canonical) => {
                    if !canonical.starts_with(base) {
                        return Err(invalid!("Path escapes the destination directory"));
                    }
                    #[cfg(windows)]
                    for component in rest.components() {
                        if let std::path::Component::Normal(s) = component
                            && let Some(s_str) = s.to_str()
                            && crate::path::is_windows_invalid_component(s_str)
                        {
                            return Err(invalid!(
                                "Symlink component is invalid or reserved on Windows: {:?}",
                                s_str
                            ));
                        }
                    }
                    Some(canonical)
                }
                Err(_) => Some(crate::path::resolve_enclosed(
                    base,
                    start,
                    rest.components().map(|c| c.as_os_str()),
                    false,
                    true,
                )?),
            };
            #[cfg(unix)]
            let _pin_target_parent = canonical_in_base
                .as_deref()
                .and_then(|c| c.parent())
                .and_then(|p| open_dir_beneath(base, p, false).ok());
            #[cfg(windows)]
            let _pin_target_parent = canonical_in_base
                .as_deref()
                .and_then(|c| c.parent())
                .and_then(|p| open_dir_beneath_windows(base, p, false).ok());
            return make_symlink_impl(outpath, target_str, existing_files, Some(base));
        }
    } else {
        // Pin any already-existing ancestor directories of `start` and the validated target
        // inside `base` across `resolve_enclosed` + `make_symlink_impl` to prevent concurrent swaps.
        #[cfg(unix)]
        let _pin_link_parent = open_dir_beneath(base, outpath.parent().unwrap_or(base), false).ok();
        #[cfg(windows)]
        let _pin_link_parent =
            open_dir_beneath_windows(base, outpath.parent().unwrap_or(base), false).ok();
        let resolved_target = crate::path::resolve_enclosed(
            base,
            start,
            rest.components().map(|c| c.as_os_str()),
            false, // only checking here, so nothing is created
            true,
        )?;
        #[cfg(unix)]
        let _pin_target_parent = resolved_target
            .parent()
            .and_then(|p| open_dir_beneath(base, p, false).ok());
        #[cfg(windows)]
        let _pin_target_parent = resolved_target
            .parent()
            .and_then(|p| open_dir_beneath_windows(base, p, false).ok());
        return make_symlink_impl(outpath, target_str, existing_files, Some(base));
    }

    make_symlink_impl(outpath, target_str, existing_files, Some(base))
}

/// Wasm and others
#[cfg(not(any(windows, unix)))]
pub(crate) fn make_symlink<T>(
    // No symlink is created on these targets, so there is nothing to contain.
    base: &Path,
    outpath: &Path,
    target: &[u8],
    #[cfg_attr(not(any(windows, unix)), allow(unused))] existing_files: &IndexMap<Box<[u8]>, T>,
    _symlink_action: SymlinkExtractAction,
) -> ZipResult<()> {
    make_symlink_as_file_utf8(outpath, target, None, Some(base))?;
    Ok(())
}

pub(crate) fn make_symlink_as_file_utf8(
    outpath: &Path,
    target: &[u8],
    unix_mode: Option<u32>,
    destination: Option<&Path>,
) -> ZipResult<()> {
    let Ok(_) = std::str::from_utf8(target) else {
        return Err(invalid!("Invalid UTF-8 as symlink target"));
    };
    make_symlink_as_file(outpath, target, unix_mode, destination)?;
    Ok(())
}

#[cfg(windows)]
fn delete_by_handle(file: std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX,
        FileDispositionInfo, FileDispositionInfoEx, SetFileInformationByHandle,
    };

    let handle = file.as_raw_handle() as _;
    let mut disp_ex = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    let ex_success = unsafe {
        SetFileInformationByHandle(
            handle,
            FileDispositionInfoEx,
            &mut disp_ex as *mut _ as *const _,
            core::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    };
    let success = if ex_success != 0 {
        ex_success
    } else {
        let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        unsafe {
            SetFileInformationByHandle(
                handle,
                FileDispositionInfo,
                &mut disposition as *mut _ as *const _,
                core::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        }
    };
    let err = if success == 0 {
        Some(std::io::Error::last_os_error())
    } else {
        None
    };
    drop(file);
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(windows)]
fn windows_case_key(path: &Path) -> std::path::PathBuf {
    // Windows NTFS `$UpCase` only folds characters 1-to-1; never expand multi-char
    // Unicode mappings (e.g. `ß` -> `SS` or `İ` -> `I\u{307}`), which refer to distinct
    // files on NTFS.
    let s = path.to_string_lossy();
    let mut folded = String::with_capacity(s.len());
    for c in s.chars() {
        let mut upper = c.to_uppercase();
        if upper.len() == 1
            && let Some(u) = upper.next()
        {
            folded.push(u);
        } else {
            folded.push(c);
        }
    }
    std::path::PathBuf::from(folded)
}

/// Normalize path keys across case-insensitive platforms (Windows) for permission and alias tracking.
pub(crate) fn normalized_path_key(path: &Path) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        windows_case_key(path)
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

#[cfg(windows)]
const fn is_disallowed_reparse_tag(tag: u32) -> bool {
    // 1. Always reject every name-surrogate reparse point (`IsReparseTagNameSurrogate`, bit 29):
    //    symlinks (`0xA000_000C`), junctions/mount points (`0xA000_0003`), WSL symlinks (`0xA000_001D`), etc.
    if (tag & 0x2000_0000) != 0 {
        return true;
    }
    // 2. Deny non-surrogate tags by default; allowlist only Microsoft's known non-redirecting
    //    cloud/sync/storage/tiering/compression tags so `APPEXECLINK` (`0x8000_001B`),
    //    `DFS` (`0x8000_000A`), `DFSR` (`0x8000_0012`), `NFS` (`0x8000_0014`),
    //    `AF_UNIX` (`0x8000_0023`), `PROJFS` (`0x9000_001C`), `WIM` (`0x8000_0008`),
    //    `WCI` (`0x8000_0018`), `WCI_1` (`0x9000_1018`), `CSV` (`0x8000_0009`),
    //    and custom 3rd-party reparse tags (`0x0000_xxxx`) cannot bypass the check.
    let is_cloud_files = (tag & !0x0000_F000) == 0x9000_001A; // IO_REPARSE_TAG_CLOUD..CLOUD_F
    let is_allowed_storage_filter = matches!(
        tag,
        0x8000_0021 // IO_REPARSE_TAG_ONEDRIVE
            | 0x8000_0015 // IO_REPARSE_TAG_FILE_PLACEHOLDER
            | 0x8000_0017 // IO_REPARSE_TAG_WOF
            | 0x8000_0013 // IO_REPARSE_TAG_DEDUP
            | 0x8000_001E // IO_REPARSE_TAG_STORAGE_SYNC (Azure File Sync)
            | 0x9000_0027 // IO_REPARSE_TAG_STORAGE_SYNC_FOLDER (Azure File Sync dir)
            | 0x8000_0006 // IO_REPARSE_TAG_HSM2
            | 0xC000_0004 // IO_REPARSE_TAG_HSM
            | 0x8000_0007 // IO_REPARSE_TAG_SIS
            | 0xC000_0014 // IO_REPARSE_TAG_APPXSTRM
    );
    !(is_cloud_files || is_allowed_storage_filter)
}

#[cfg(windows)]
fn query_reparse_tag(handle: std::os::windows::io::RawHandle) -> std::io::Result<u32> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_TAG_INFO, FileAttributeTagInfo, GetFileInformationByHandleEx,
    };

    let mut tag_info = unsafe { core::mem::zeroed::<FILE_ATTRIBUTE_TAG_INFO>() };
    let size = u32::try_from(core::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>())
        .map_err(|_| std::io::Error::other("FILE_ATTRIBUTE_TAG_INFO size overflow"))?;
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle as _,
            FileAttributeTagInfo,
            &mut tag_info as *mut _ as *mut _,
            size,
        )
    };
    if ok == 0 {
        // Fail closed if a reparse point's tag cannot be inspected.
        return Err(std::io::Error::last_os_error());
    }
    Ok(tag_info.ReparseTag)
}

#[cfg(windows)]
fn is_disallowed_reparse_handle(
    handle: std::os::windows::io::RawHandle,
    file_attributes: u32,
) -> std::io::Result<bool> {
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    if (file_attributes & FILE_ATTRIBUTE_REPARSE_POINT) == 0 {
        return Ok(false);
    }
    Ok(is_disallowed_reparse_tag(query_reparse_tag(handle)?))
}

#[cfg(windows)]
fn prepare_leaf_overwrite_windows(outpath: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_OFFLINE,
        FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        GetFileInformationByHandle,
    };

    let Ok(meta) = std::fs::symlink_metadata(outpath) else {
        return Ok(());
    };
    if meta.file_type().is_symlink() {
        // Directory junctions and directory symlinks carry the directory attribute and
        // require `remove_dir`, while file symlinks require `remove_file`.
        if (meta.file_attributes() & FILE_ATTRIBUTE_DIRECTORY) != 0 {
            return std::fs::remove_dir(outpath);
        }
        return std::fs::remove_file(outpath);
    }

    // Open with DELETE | FILE_READ_ATTRIBUTES to inspect link count/reparse state and
    // atomically delete by handle when needed; fall back to `FILE_READ_ATTRIBUTES` when
    // `DELETE` is blocked by an ACL or external reader lacking `FILE_SHARE_DELETE`,
    // so ordinary `nNumberOfLinks == 1` overwrites still succeed.
    let (f, has_delete) = match std::fs::OpenOptions::new()
        .access_mode(DELETE | FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(outpath)
    {
        Ok(f) => (f, true),
        Err(e)
            if e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(32) =>
        {
            let ro = std::fs::OpenOptions::new()
                .access_mode(FILE_READ_ATTRIBUTES)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(outpath)
                .map_err(|_| e)?;
            (ro, false)
        }
        Err(e) => return Err(e),
    };
    let mut info = unsafe { core::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    if unsafe { GetFileInformationByHandle(f.as_raw_handle() as _, &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Cannot overwrite an existing directory with an extracted file",
        ));
    }
    let has_reparse = (info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
    let reparse_tag = if has_reparse {
        Some(query_reparse_tag(f.as_raw_handle())?)
    } else {
        None
    };
    if let Some(tag) = reparse_tag
        && is_disallowed_reparse_tag(tag)
    {
        // Leaf file name-surrogate links that `std::fs::FileType::is_symlink` does not
        // classify (e.g. LX/global symlinks) are safely replaced like symlinks; other
        // non-surrogate system tags (`APPEXECLINK`, `AF_UNIX`, `DFS`, `WIM`) stay rejected.
        if (tag & 0x2000_0000) != 0 && has_delete {
            return delete_by_handle(f);
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Extracted file cannot be written through a symbolic link or disallowed reparse point",
        ));
    }
    // Unlink dehydrated cloud/tiering placeholders (`RECALL_ON_DATA_ACCESS` / `OFFLINE`)
    // before `create_outfile` opens with `FILE_FLAG_OPEN_REPARSE_POINT` and calls
    // `set_len(0)`, avoiding `cldflt.sys` errors while replacing the stub cleanly.
    let is_offline_placeholder = has_reparse
        && (info.dwFileAttributes
            & (FILE_ATTRIBUTE_OFFLINE
                | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
                | FILE_ATTRIBUTE_RECALL_ON_OPEN))
            != 0;
    if info.nNumberOfLinks > 1 || is_offline_placeholder {
        if !has_delete {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Cannot unlink existing hard link or cloud placeholder without DELETE access",
            ));
        }
        delete_by_handle(f)?;
    }
    Ok(())
}

/// Remove a partially-written output file anchored beneath `dest` without following
/// any concurrently-swapped intermediate directory symlinks or junctions.
pub(crate) fn remove_outfile_beneath(
    dest: &Path,
    outpath: &Path,
    outfile: std::fs::File,
) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::{Error, ErrorKind};
        use std::os::fd::AsRawFd;

        let mut open_st: libc::stat = unsafe { core::mem::zeroed() };
        if unsafe { libc::fstat(outfile.as_raw_fd(), &mut open_st) } != 0 {
            return Err(Error::last_os_error());
        }
        let parent = outpath.parent().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                "Output file path has no parent directory",
            )
        })?;
        let file_name = outpath.file_name().ok_or_else(|| {
            Error::new(ErrorKind::InvalidInput, "Output file path has no file name")
        })?;
        let dirfd = open_dir_beneath(dest, parent, false)?;
        let name = cstr(file_name)?;
        let mut entry_st: libc::stat = unsafe { core::mem::zeroed() };
        if unsafe {
            libc::fstatat(
                dirfd.as_raw_fd(),
                name.as_ptr(),
                &mut entry_st,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(Error::last_os_error());
        }
        if (stat_mode(&entry_st) & libc::S_IFMT) != libc::S_IFREG
            || entry_st.st_dev != open_st.st_dev
            || entry_st.st_ino != open_st.st_ino
        {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "Refusing to remove unverified output path after extraction failure",
            ));
        }
        if unsafe { libc::unlinkat(dirfd.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(Error::last_os_error());
        }
        drop(outfile);
        Ok(())
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, GetFileInformationByHandle,
        };
        if let Some(parent) = outpath.parent() {
            let _pinned = open_dir_beneath_windows(dest, parent, false)?;
            let mut expected = unsafe { core::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
            let have_expected =
                unsafe { GetFileInformationByHandle(outfile.as_raw_handle() as _, &mut expected) }
                    != 0;
            match delete_by_handle(outfile) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && have_expected => {
                    let reopened = std::fs::OpenOptions::new()
                        .access_mode(DELETE | FILE_READ_ATTRIBUTES)
                        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                        .open(outpath)?;
                    let mut actual = unsafe { core::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
                    if unsafe {
                        GetFileInformationByHandle(reopened.as_raw_handle() as _, &mut actual)
                    } == 0
                    {
                        return Err(std::io::Error::last_os_error());
                    }
                    if (actual.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0
                        || is_disallowed_reparse_handle(
                            reopened.as_raw_handle(),
                            actual.dwFileAttributes,
                        )?
                        || actual.dwVolumeSerialNumber != expected.dwVolumeSerialNumber
                        || actual.nFileIndexHigh != expected.nFileIndexHigh
                        || actual.nFileIndexLow != expected.nFileIndexLow
                    {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "Refusing to remove unverified output path after extraction failure",
                        ));
                    }
                    return delete_by_handle(reopened);
                }
                Err(e) => return Err(e),
            }
        }
        drop(outfile);
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = dest;
        {
            let _closed = outfile;
        }
        std::fs::remove_file(outpath)
    }
}

/// Remove a partially-written output file anchored beneath `dest` and warn on `stderr`
/// for any cleanup error other than `NotFound`.
pub(crate) fn cleanup_partial_outfile_beneath(dest: &Path, outpath: &Path, outfile: std::fs::File) {
    if let Err(err) = remove_outfile_beneath(dest, outpath, outfile)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!(
            "extract: failed to remove unauthenticated partial file {}: {err:?}",
            outpath.display()
        );
    }
}

/// Move or copy a staged file (or staged symlink) from `from` (inside `staging_root`)
/// to `outpath` beneath `dest` while pinning all destination parent directories.
#[cfg_attr(not(any(unix, windows)), allow(unused_variables))]
// `staging_root` and `dest` are only needed on platforms with dirfd/handle anchoring.
pub(crate) fn move_staged_file_beneath(
    dest: &Path,
    staging_root: &Path,
    from: &Path,
    outpath: &Path,
) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::{Error, ErrorKind};
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

        let from_parent = from.parent().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                "Staged path has no parent directory",
            )
        })?;
        let from_name = from
            .file_name()
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "Staged path has no file name"))?;
        let to_parent = outpath.parent().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                "Output path has no parent directory",
            )
        })?;
        let to_name = outpath
            .file_name()
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "Output path has no file name"))?;

        let from_dirfd = open_dir_beneath(staging_root, from_parent, false)?;
        let to_dirfd = open_dir_beneath(dest, to_parent, true)?;
        let from_c = cstr(from_name)?;
        let to_c = cstr(to_name)?;

        let r = unsafe {
            libc::renameat(
                from_dirfd.as_raw_fd(),
                from_c.as_ptr(),
                to_dirfd.as_raw_fd(),
                to_c.as_ptr(),
            )
        };
        if r == 0 {
            return Ok(());
        }
        let err = Error::last_os_error();
        if err.kind() != ErrorKind::CrossesDevices {
            return Err(err);
        }

        // Cross-device fallback anchored at both `from_dirfd` and `to_dirfd` without
        // falling back to path-based `std::fs::copy`.
        let mut from_st: libc::stat = unsafe { core::mem::zeroed() };
        if unsafe {
            libc::fstatat(
                from_dirfd.as_raw_fd(),
                from_c.as_ptr(),
                &mut from_st,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(Error::last_os_error());
        }
        if (stat_mode(&from_st) & libc::S_IFMT) == libc::S_IFLNK {
            let target = std::fs::read_link(from)?;
            let target_c = cstr(target.as_os_str())?;
            let r_unlink = unsafe { libc::unlinkat(to_dirfd.as_raw_fd(), to_c.as_ptr(), 0) };
            if r_unlink != 0 {
                let unlink_err = Error::last_os_error();
                if unlink_err.raw_os_error() != Some(libc::ENOENT) {
                    return Err(unlink_err);
                }
            }
            if unsafe { libc::symlinkat(target_c.as_ptr(), to_dirfd.as_raw_fd(), to_c.as_ptr()) }
                != 0
            {
                return Err(Error::last_os_error());
            }
            let _ = unsafe { libc::unlinkat(from_dirfd.as_raw_fd(), from_c.as_ptr(), 0) };
            return Ok(());
        }

        let src_raw = unsafe {
            libc::openat(
                from_dirfd.as_raw_fd(),
                from_c.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if src_raw < 0 {
            return Err(Error::last_os_error());
        }
        let src_fd = unsafe { OwnedFd::from_raw_fd(src_raw) };
        let mut src_st: libc::stat = unsafe { core::mem::zeroed() };
        if unsafe { libc::fstat(src_fd.as_raw_fd(), &mut src_st) } != 0 {
            return Err(Error::last_os_error());
        }
        if (stat_mode(&src_st) & libc::S_IFMT) != libc::S_IFREG {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "Staged entry is not a regular file",
            ));
        }
        let mut src_file = std::fs::File::from(src_fd);
        let mut dst_file = create_outfile_beneath(dest, outpath, None)?;
        if let Err(e) = std::io::copy(&mut src_file, &mut dst_file) {
            cleanup_partial_outfile_beneath(dest, outpath, dst_file);
            return Err(e);
        }
        drop(src_file);
        let _ = unsafe { libc::unlinkat(from_dirfd.as_raw_fd(), from_c.as_ptr(), 0) };
        Ok(())
    }

    #[cfg(windows)]
    {
        let _ = staging_root;
        let to_parent = outpath.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Output path has no parent directory",
            )
        })?;
        let pinned = open_dir_beneath_windows(dest, to_parent, true)?;
        prepare_leaf_overwrite_windows(outpath)?;

        let copy_into_pinned = || -> std::io::Result<()> {
            use std::os::windows::fs::MetadataExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
            let from_meta = std::fs::symlink_metadata(from)?;
            if from_meta.file_type().is_symlink() {
                let target = std::fs::read_link(from)?;
                if (from_meta.file_attributes() & FILE_ATTRIBUTE_DIRECTORY) != 0 {
                    std::os::windows::fs::symlink_dir(&target, outpath)?;
                    let _ = std::fs::remove_dir(from);
                } else {
                    std::os::windows::fs::symlink_file(&target, outpath)?;
                    let _ = std::fs::remove_file(from);
                }
                return Ok(());
            }
            let mut dst_file = create_outfile(outpath, None, Some(dest))?;
            {
                let mut src_file = std::fs::File::open(from)?;
                if let Err(e) = std::io::copy(&mut src_file, &mut dst_file) {
                    drop(src_file);
                    cleanup_partial_outfile_beneath(dest, outpath, dst_file);
                    return Err(e);
                }
            }
            let _ = std::fs::remove_file(from);
            Ok(())
        };

        // Holding ancestor directory handles without `FILE_SHARE_WRITE` blocks reparse-point
        // swaps, which also causes `MoveFileExW` (`fs::rename`) into a non-empty pinned
        // `to_parent` to fail with sharing violation (code 32). Stream through `create_outfile`
        // while keeping `pinned` alive so ancestor verification holds through completion.
        if pinned.is_empty() {
            match std::fs::rename(from, outpath) {
                Ok(()) => Ok(()),
                Err(e)
                    if e.kind() == std::io::ErrorKind::CrossesDevices
                        || e.raw_os_error() == Some(32) =>
                {
                    copy_into_pinned()
                }
                Err(e) => Err(e),
            }
        } else {
            copy_into_pinned()
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        if let Err(e) = std::fs::rename(from, outpath) {
            if e.kind() != std::io::ErrorKind::CrossesDevices {
                return Err(e);
            }
            let mut dst_file = create_outfile(outpath, None, Some(dest))?;
            {
                let mut src_file = std::fs::File::open(from)?;
                std::io::copy(&mut src_file, &mut dst_file)?;
            }
            let _ = std::fs::remove_file(from);
        }
        Ok(())
    }
}

/// Apply the read-only flag on `path` beneath `dest` on non-Unix platforms without
/// following symbolic links or NTFS junctions.
#[cfg(not(unix))]
#[cfg_attr(not(windows), allow(unused_variables))]
// `dest` is used on Windows to pin ancestor directories against junction swaps.
pub(crate) fn set_readonly_beneath(dest: &Path, path: &Path) -> std::io::Result<()> {
    if path == dest {
        return Ok(());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_READ_ATTRIBUTES, FILE_WRITE_ATTRIBUTES, GetFileInformationByHandle,
        };

        let Some(parent) = path.parent() else {
            return Ok(());
        };
        let _pinned = open_dir_beneath_windows(dest, parent, false)?;
        let handle = std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let mut info = unsafe { core::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        if unsafe { GetFileInformationByHandle(handle.as_raw_handle() as _, &mut info) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if is_disallowed_reparse_handle(handle.as_raw_handle(), info.dwFileAttributes)? {
            return Ok(());
        }
        let mut perms = handle.metadata()?.permissions();
        perms.set_readonly(true);
        handle.set_permissions(perms)
    }
    #[cfg(not(windows))]
    {
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.file_type().is_symlink() {
            let mut perms = metadata.permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(path, perms)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn create_outfile_beneath(
    dest: &Path,
    outpath: &Path,
    unix_mode: Option<u32>,
) -> std::io::Result<std::fs::File> {
    use std::io::{Error, ErrorKind};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    let parent = outpath.parent().ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidInput,
            "Output file path has no parent directory",
        )
    })?;
    let file_name = outpath
        .file_name()
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "Output file path has no file name"))?;

    let dirfd = open_dir_beneath(dest, parent, true)?;
    let name = cstr(file_name)?;

    // If the leaf already exists inside the validated parent directory as a symlink
    // or a hard link (nlink > 1), unlink it relative to `dirfd` before creating the
    // new file so that existing hard-link targets are never truncated or overwritten.
    let mut pre_st: libc::stat = unsafe { core::mem::zeroed() };
    if unsafe {
        libc::fstatat(
            dirfd.as_raw_fd(),
            name.as_ptr(),
            &mut pre_st,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } == 0
        && ((stat_mode(&pre_st) & libc::S_IFMT) == libc::S_IFLNK || pre_st.st_nlink > 1)
    {
        let r = unsafe { libc::unlinkat(dirfd.as_raw_fd(), name.as_ptr(), 0) };
        if r != 0 {
            let e = Error::last_os_error();
            if e.raw_os_error() != Some(libc::ENOENT) {
                return Err(e);
            }
        }
    }

    // Open/create the file beneath `dirfd` with O_NOFOLLOW, O_NONBLOCK (so a pre-existing
    // or raced FIFO never blocks), and WITHOUT O_TRUNC. Truncation is deferred until after
    // `fstat` on the opened descriptor confirms it is a regular file with link count <= 1.
    let mode: libc::c_uint = unix_mode.unwrap_or(0o666) & 0o777;
    let raw = unsafe {
        libc::openat(
            dirfd.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            mode,
        )
    };
    if raw < 0 {
        let e = Error::last_os_error();
        return Err(if is_nofollow_error(&e) {
            Error::new(
                ErrorKind::PermissionDenied,
                "Extracted file cannot be written through a symbolic link",
            )
        } else {
            e
        });
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };

    let mut st: libc::stat = unsafe { core::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(Error::last_os_error());
    }
    if (stat_mode(&st) & libc::S_IFMT) != libc::S_IFREG {
        return Err(Error::new(
            ErrorKind::PermissionDenied,
            "Extracted file cannot be written to a non-regular file",
        ));
    }
    if st.st_nlink > 1 {
        return Err(Error::new(
            ErrorKind::PermissionDenied,
            "Extracted file cannot be written through a hard link",
        ));
    }

    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags >= 0 && (flags & libc::O_NONBLOCK) != 0 {
        let _ = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) };
    }

    let file = std::fs::File::from(fd);
    file.set_len(0)?;
    Ok(file)
}

/// Create an output file for extraction securely.
///
/// If `outpath` already exists as a symbolic link or hard link, it is removed before creating the regular
/// file so that extraction will not follow the link and overwrite or truncate a target file outside the
/// destination directory. After creation, the file is verified to ensure it was not swapped
/// for a symbolic link or hard link.
#[cfg_attr(not(unix), allow(unused_variables))]
// `unix_mode` only applies on Unix targets (`create_outfile_beneath` / `OpenOptions::mode`).
pub(crate) fn create_outfile(
    outpath: &Path,
    unix_mode: Option<u32>,
    destination: Option<&Path>,
) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    if let Some(dest) = destination {
        return create_outfile_beneath(dest, outpath, unix_mode);
    }

    #[cfg(windows)]
    let _pinned_ancestors = if let Some(dest) = destination
        && let Some(parent) = outpath.parent()
    {
        Some(open_dir_beneath_windows(dest, parent, true)?)
    } else {
        None
    };

    #[cfg(windows)]
    prepare_leaf_overwrite_windows(outpath)?;

    #[cfg(not(windows))]
    if let Ok(meta) = std::fs::symlink_metadata(outpath) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.file_type().is_symlink() || meta.nlink() > 1 {
                std::fs::remove_file(outpath)?;
            }
        }
        #[cfg(not(unix))]
        {
            if meta.file_type().is_symlink() {
                std::fs::remove_file(outpath)?;
            }
        }
    }

    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(mode) = unix_mode {
            options.mode(mode);
        }
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_SEQUENTIAL_SCAN, FILE_GENERIC_WRITE,
            FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
        };
        options
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN);
        match options
            .access_mode(FILE_GENERIC_WRITE | FILE_READ_ATTRIBUTES | DELETE)
            .open(outpath)
        {
            Ok(f) => f,
            Err(e)
                if e.kind() == std::io::ErrorKind::PermissionDenied
                    || e.raw_os_error() == Some(32) =>
            {
                // External apps holding `FILE_SHARE_READ | FILE_SHARE_WRITE` (without
                // `FILE_SHARE_DELETE`) or write-without-delete ACLs reject `DELETE` on open;
                // retry without `DELETE` so ordinary overwrites still succeed.
                options
                    .access_mode(FILE_GENERIC_WRITE | FILE_READ_ATTRIBUTES)
                    .open(outpath)?
            }
            Err(e) => return Err(e),
        }
    };
    #[cfg(not(windows))]
    let file = options.open(outpath)?;

    if file.metadata()?.file_type().is_symlink()
        || std::fs::symlink_metadata(outpath).is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Extracted file cannot be written through a symbolic link",
        ));
    }

    // Validate that no ancestor directory component inside destination has been swapped for a symbolic link.
    // Traversal stops once the extraction destination is reached, since the destination path itself
    // may legitimately reside under or be a symbolic link (e.g. /tmp or /var on macOS/Linux).
    let mut current = outpath.parent();
    while let Some(parent) = current {
        if let Some(dest) = destination
            && (parent == dest
                || parent.components().eq(dest.components())
                || !parent.starts_with(dest))
        {
            break;
        }
        if std::fs::symlink_metadata(parent)
            .is_ok_and(|parent_meta| parent_meta.file_type().is_symlink())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Extracted file parent directory cannot be a symbolic link",
            ));
        }
        current = parent.parent();
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if file.metadata()?.nlink() > 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Extracted file cannot be written through a hard link",
            ));
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_TYPE_DISK,
            GetFileInformationByHandle, GetFileType,
        };
        unsafe {
            let mut info = core::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>();
            if GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if GetFileType(file.as_raw_handle() as _) != FILE_TYPE_DISK
                || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Extracted file must be a regular disk file",
                ));
            }
            if is_disallowed_reparse_handle(file.as_raw_handle(), info.dwFileAttributes)? {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Extracted file cannot be written through a symbolic link or disallowed reparse point",
                ));
            }
            if info.nNumberOfLinks > 1 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Extracted file cannot be written through a hard link",
                ));
            }
        }
    }

    file.set_len(0)?;
    Ok(file)
}

pub(crate) fn make_symlink_as_file(
    outpath: &Path,
    target: &[u8],
    unix_mode: Option<u32>,
    destination: Option<&Path>,
) -> std::io::Result<()> {
    use std::io::Write;
    let mut output = create_outfile(outpath, unix_mode, destination)?;
    if let Err(e) = output.write_all(target) {
        if let Some(dest) = destination {
            cleanup_partial_outfile_beneath(dest, outpath, output);
        }
        return Err(e);
    }
    Ok(())
}

#[cfg(unix)]
fn set_permissions_dirfd_entry(
    dirfd: &std::os::fd::OwnedFd,
    name: &std::ffi::CStr,
    mode: std::fs::Permissions,
) -> std::io::Result<()> {
    use std::io::Error;
    use std::os::fd::AsRawFd;

    // SAFETY: `libc::stat` is a plain C struct where an all-zero bit pattern is valid.
    let mut st: libc::stat = unsafe { core::mem::zeroed() };
    // SAFETY: `dirfd` is a valid open directory descriptor, `name` is a valid NUL-terminated
    // C string, and `&mut st` points to a properly aligned writable `libc::stat` buffer.
    if unsafe {
        libc::fstatat(
            dirfd.as_raw_fd(),
            name.as_ptr(),
            &mut st,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(Error::last_os_error());
    }

    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        use std::os::unix::fs::PermissionsExt;

        let raw_mode = libc::mode_t::from((mode.mode() & 0o7777) as u16);
        // SAFETY: `dirfd` is a valid open directory descriptor and `name` is a valid
        // NUL-terminated C string.
        if unsafe {
            libc::fchmodat(
                dirfd.as_raw_fd(),
                name.as_ptr(),
                raw_mode,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(Error::last_os_error());
        }
        Ok(())
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )))]
    {
        use std::io::ErrorKind;
        use std::os::fd::{FromRawFd, OwnedFd};

        if (stat_mode(&st) & libc::S_IFMT) == libc::S_IFLNK {
            return Ok(());
        }

        let is_dir = (stat_mode(&st) & libc::S_IFMT) == libc::S_IFDIR;
        let base_flags = if is_dir {
            libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
        } else {
            libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC
        };

        let mut raw = unsafe {
            libc::openat(
                dirfd.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | base_flags,
            )
        };
        if raw < 0 && !is_dir {
            let err = Error::last_os_error();
            if is_nofollow_error(&err) {
                return Err(Error::new(
                    ErrorKind::PermissionDenied,
                    "Cannot set permissions through a symbolic link",
                ));
            }
            if err.raw_os_error() == Some(libc::EACCES) {
                raw = unsafe {
                    libc::openat(
                        dirfd.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_WRONLY | base_flags,
                    )
                };
            } else {
                return Err(err);
            }
        }
        if raw >= 0 {
            let file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(raw) });
            return file.set_permissions(mode);
        }

        let err = Error::last_os_error();
        if is_nofollow_error(&err) {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "Cannot set permissions through a symbolic link",
            ));
        }

        #[cfg(any(target_os = "linux", target_os = "android"))]
        if err.raw_os_error() == Some(libc::EACCES) {
            use std::os::unix::fs::PermissionsExt;

            // `O_RDONLY`/`O_WRONLY` fail with `EACCES` on non-readable/non-writable inodes
            // (e.g., `0o000` or `0o100`). Open with `O_PATH | O_NOFOLLOW`, verify via `fstat`
            // that the pinned descriptor is not a symlink, and apply `fchmodat` on
            // `/proc/self/fd/<fd>` (which operates on the pinned inode rather than walking `dirfd`).
            let path_raw = unsafe {
                libc::openat(
                    dirfd.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if path_raw < 0 {
                let path_err = Error::last_os_error();
                return Err(if is_nofollow_error(&path_err) {
                    Error::new(
                        ErrorKind::PermissionDenied,
                        "Cannot set permissions through a symbolic link",
                    )
                } else {
                    path_err
                });
            }
            let path_fd = unsafe { OwnedFd::from_raw_fd(path_raw) };
            let mut fd_st: libc::stat = unsafe { core::mem::zeroed() };
            if unsafe { libc::fstat(path_fd.as_raw_fd(), &mut fd_st) } != 0 {
                return Err(Error::last_os_error());
            }
            if (stat_mode(&fd_st) & libc::S_IFMT) == libc::S_IFLNK {
                return Err(Error::new(
                    ErrorKind::PermissionDenied,
                    "Cannot set permissions through a symbolic link",
                ));
            }
            let raw_mode = libc::mode_t::from((mode.mode() & 0o7777) as u16);
            let proc_path =
                std::ffi::CString::new(format!("/proc/self/fd/{}", path_fd.as_raw_fd()))
                    .map_err(|_| Error::new(ErrorKind::InvalidInput, "NUL byte in fd path"))?;
            if unsafe { libc::fchmodat(libc::AT_FDCWD, proc_path.as_ptr(), raw_mode, 0) } != 0 {
                return Err(Error::last_os_error());
            }
            return Ok(());
        }

        // Never fall back to `fchmodat(dirfd, name, raw_mode, 0)` because `flags = 0` follows symlinks.
        Err(err)
    }
}

#[cfg(unix)]
pub(crate) fn set_permissions_no_follow_symlink(
    path: &Path,
    mode: std::fs::Permissions,
    destination: Option<&Path>,
) -> std::io::Result<()> {
    use std::fs;
    if let Some(dest) = destination {
        if path == dest {
            // Never let archive metadata (such as `./`) alter the caller's root destination directory.
            return Ok(());
        }
        if let (Some(parent), Some(file_name)) = (path.parent(), path.file_name()) {
            let dirfd = open_dir_beneath(dest, parent, false)?;
            let name = cstr(file_name)?;
            return set_permissions_dirfd_entry(&dirfd, &name, mode);
        }
    }

    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
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
            let raw_mode = nix::libc::mode_t::from((mode.mode() & 0o7777) as u16);
            let mode = Mode::from_bits_truncate(raw_mode);

            // Use fchmodat with the flag telling it NOT to follow the symlink
            fchmodat(AT_FDCWD, path, mode, FchmodatFlags::NoFollowSymlink)?;
        }
        Ok(())
    } else {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = fs::OpenOptions::new();
        opts.read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        if meta.is_dir() {
            opts.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY);
        }
        match opts.open(path) {
            Ok(file) => file.set_permissions(mode),
            Err(err) => Err(if is_nofollow_error(&err) {
                std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Cannot set permissions through a symbolic link",
                )
            } else {
                err
            }),
        }
    }
}

/// Store all entries which specify a numeric "mode" which is familiar to POSIX operating systems.
#[derive(Default, Debug)]
struct UnixFileModes {
    #[cfg(unix)]
    map: std::collections::BTreeMap<std::path::PathBuf, u32>,
    #[cfg(not(unix))]
    read_only_map: std::collections::BTreeMap<std::path::PathBuf, std::path::PathBuf>,
}

impl UnixFileModes {
    pub fn add_mode(&mut self, path: std::path::PathBuf, mode: u32) {
        // We don't print a warning or consider it remotely out of the ordinary to receive two
        // separate modes for the same path (e.g. entries named `a` and `./a`): just take the
        // later one, as the later file's contents are what's on disk.
        #[cfg(unix)]
        self.map.insert(path, mode);
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            let key = windows_case_key(&path);
            #[cfg(not(windows))]
            let key = path.clone();
            if mode & 0o222 == 0 {
                self.read_only_map.insert(key, path);
            } else {
                self.read_only_map.remove(&key);
            }
        }
    }

    pub fn clear_mode(&mut self, path: &Path) {
        #[cfg(unix)]
        self.map.remove(path);
        #[cfg(not(unix))]
        self.read_only_map.remove(&normalized_path_key(path));
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
        self.read_only_map.into_values().rev()
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
        self.extract_internal(directory, ExtractOptions::default())
    }

    /// Same as [`ZipArchive::extract`], but with custom [`ExtractOptions`].
    ///
    /// To unwrap a root directory, set a filter with [`ExtractOptions::with_root_dir_filter`];
    /// see [`ZipArchive::extract_unwrapped_root_dir`] for the semantics.
    pub fn extract_with_options<P: AsRef<Path>>(
        &mut self,
        directory: P,
        extract_options: ExtractOptions<impl RootDirFilter>,
    ) -> ZipResult<()> {
        self.extract_internal(directory, extract_options)
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
    /// To combine unwrapping with other options, use [`ZipArchive::extract_with_options`]
    /// with [`ExtractOptions::with_root_dir_filter`].
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
        self.extract_internal(
            directory,
            ExtractOptions::default().with_root_dir_filter(root_dir_filter),
        )
    }

    fn pre_extract_checks(
        &mut self,
        options: &ExtractOptions<impl RootDirFilter>,
    ) -> ZipResult<()> {
        if options.enforce_header_consistency {
            self.validate_header_consistency()?;
        }

        if let Some(limit) = options.size_limit {
            match self.decompressed_size() {
                Some(size) if size > u128::from(limit) => {
                    return Err(ZipError::DecompressedSizeLimitExceeded { size, limit });
                }
                None => return Err(ZipError::DecompressedSizeUnknown),
                Some(_) => {}
            }
        }

        Ok(())
    }

    fn extract_internal<P: AsRef<Path>>(
        &mut self,
        directory: P,
        extract_options: ExtractOptions<impl RootDirFilter>,
    ) -> ZipResult<()> {
        use std::fs;

        self.pre_extract_checks(&extract_options)?;

        let ExtractOptions {
            symlink_action,
            enforce_header_consistency,
            root_dir_filter,
            ..
        } = extract_options;

        #[allow(clippy::disallowed_methods)]
        // Creates the caller-chosen extraction root itself before canonicalizing it; no untrusted archive path is involved.
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
                .enforce_header_consistency(enforce_header_consistency);
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
                match symlink_action {
                    SymlinkExtractAction::NoExtract => {
                        // do nothing
                    }
                    SymlinkExtractAction::ExtractAsFile => {
                        let writable_mode = unix_mode.map(|_| 0o600);
                        make_symlink_as_file(
                            &outpath,
                            &target,
                            writable_mode,
                            Some(directory.as_ref()),
                        )?;
                        if let Some(m) = unix_mode {
                            files_by_unix_mode.add_mode(outpath, m);
                        } else {
                            files_by_unix_mode.clear_mode(&outpath);
                        }
                    }
                    SymlinkExtractAction::ExtractAsFileUtf8 => {
                        let writable_mode = unix_mode.map(|_| 0o600);
                        make_symlink_as_file_utf8(
                            &outpath,
                            &target,
                            writable_mode,
                            Some(directory.as_ref()),
                        )?;
                        if let Some(m) = unix_mode {
                            files_by_unix_mode.add_mode(outpath, m);
                        } else {
                            files_by_unix_mode.clear_mode(&outpath);
                        }
                    }
                    #[cfg(any(unix, windows))]
                    SymlinkExtractAction::ExtractRecursiveInFolder => {
                        make_symlink(
                            directory.as_ref(),
                            &outpath,
                            &target,
                            &self.shared.files,
                            symlink_action,
                        )?;
                    }
                    #[cfg(any(unix, windows))]
                    SymlinkExtractAction::ExtractInFolder => {
                        make_symlink(
                            directory.as_ref(),
                            &outpath,
                            &target,
                            &self.shared.files,
                            symlink_action,
                        )?;
                    }
                    #[cfg(any(unix, windows))]
                    SymlinkExtractAction::ExtractNoRestrictions => {
                        let Ok(target_str) = std::str::from_utf8(&target) else {
                            return Err(invalid!("Invalid UTF-8 as symlink target"));
                        };
                        make_symlink_impl(
                            &outpath,
                            target_str,
                            &self.shared.files,
                            Some(directory.as_ref()),
                        )?;
                    }
                }
            } else if file.is_dir() {
                if created_dirs.insert(outpath.clone()) {
                    make_writable_dir_all(&outpath, Some(directory.as_ref()))?;
                }
                // Check for real permissions, which we'll set in a second pass.
                if outpath != directory
                    && let Some(mode) = file.unix_mode()
                {
                    files_by_unix_mode.add_mode(outpath, mode);
                }
            } else {
                // Keep owner r/w during extraction so duplicate entries overwriting a `0o444`/`0o000`
                // entry do not fail; the final pass applies the entry's exact mode.
                let writable_mode = file.unix_mode().map(|_| 0o600);
                let mut outfile =
                    create_outfile(&outpath, writable_mode, Some(directory.as_ref()))?;
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
                if let Err(e) = copy_res {
                    cleanup_partial_outfile_beneath(directory.as_ref(), &outpath, outfile);
                    return Err(e.into());
                }
                if let Some(mode) = file.unix_mode() {
                    // OpenOptions::mode only applies when the file is created, and read-only
                    // attributes on non-Unix must wait until after any later overwriting entry.
                    files_by_unix_mode.add_mode(outpath, mode);
                } else {
                    files_by_unix_mode.clear_mode(&outpath);
                }
                // Set original timestamp.
                #[cfg(feature = "chrono")]
                if let Some(last_modified) = file.last_modified()
                    && let Some(t) = last_modified.datetime_to_systemtime()
                {
                    outfile.set_modified(t)?;
                }
            }
        }

        // Ensure we update children's permissions before making a parent unwritable.
        #[cfg(unix)]
        for (path, perms) in files_by_unix_mode.all_perms_with_children_first() {
            set_permissions_no_follow_symlink(&path, perms, Some(directory.as_ref()))?;
        }
        #[cfg(not(unix))]
        for path in files_by_unix_mode.all_read_only_with_children_first() {
            set_readonly_beneath(directory.as_ref(), &path)?;
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
    /// If `root_dir_filter` is `Some`, it takes precedence over any filter set with
    /// [`ExtractOptions::with_root_dir_filter`].
    ///
    /// See [`ZipArchive::par_extract_with_reader_factory`] for common `reader_factory` implementations.
    pub fn par_extract_with_reader_factory_and_options<P, F, Filter>(
        &mut self,
        directory: P,
        reader_factory: F,
        extract_options: ExtractOptions<impl RootDirFilter>,
        root_dir_filter: Option<Filter>,
    ) -> ZipResult<()>
    where
        P: AsRef<Path>,
        F: Fn() -> std::io::Result<R> + Sync + Send,
        Filter: RootDirFilter,
    {
        // `root_dir_filter` predates `ExtractOptions::with_root_dir_filter`
        // and is kept for compatibility; it takes precedence when `Some`.
        // Separate calls because the arms have different filter types.
        match root_dir_filter {
            Some(filter) => self.par_extract_internal(
                directory.as_ref(),
                &reader_factory,
                extract_options.with_root_dir_filter(filter),
            ),
            None => self.par_extract_internal(directory.as_ref(), &reader_factory, extract_options),
        }
    }

    fn par_extract_internal<F, Filter>(
        &mut self,
        directory: &Path,
        reader_factory: &F,
        extract_options: ExtractOptions<Filter>,
    ) -> ZipResult<()>
    where
        F: Fn() -> std::io::Result<R> + Sync + Send,
        Filter: RootDirFilter,
    {
        self.pre_extract_checks(&extract_options)?;

        let ExtractOptions {
            symlink_action,
            enforce_header_consistency,
            root_dir_filter,
            ..
        } = extract_options;

        #[allow(clippy::disallowed_methods)]
        // Creates the caller-chosen extraction root itself before canonicalizing it; no untrusted archive path is involved.
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
        let mut regular_files: indexmap::IndexMap<std::path::PathBuf, (std::path::PathBuf, usize)> =
            indexmap::IndexMap::new();

        for i in 0..self.len() {
            let (file_name_raw, data) = self
                .shared
                .files
                .get_index(i)
                .ok_or(ZipError::FileNotFound)?;

            let outpath = crate::read::zipfile::safe_prepare_path_from_metadata(
                file_name_raw,
                data,
                directory.as_ref(),
                root_dir.as_ref(),
            )?;

            if data.is_symlink() {
                let dedup_key = normalized_path_key(&outpath);
                let read_options = crate::read::ZipReadOptions::new()
                    .enforce_header_consistency(enforce_header_consistency);
                let mut file = self.by_index_with_options(i, read_options)?;
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
                match symlink_action {
                    SymlinkExtractAction::NoExtract => {}
                    SymlinkExtractAction::ExtractAsFile => {
                        regular_files.shift_remove(&dedup_key);
                        let writable_mode = unix_mode.map(|_| 0o600);
                        make_symlink_as_file(
                            &outpath,
                            &target,
                            writable_mode,
                            Some(directory.as_ref()),
                        )?;
                        if let Some(m) = unix_mode {
                            files_by_unix_mode.add_mode(outpath, m);
                        } else {
                            files_by_unix_mode.clear_mode(&outpath);
                        }
                    }
                    SymlinkExtractAction::ExtractAsFileUtf8 => {
                        regular_files.shift_remove(&dedup_key);
                        let writable_mode = unix_mode.map(|_| 0o600);
                        make_symlink_as_file_utf8(
                            &outpath,
                            &target,
                            writable_mode,
                            Some(directory.as_ref()),
                        )?;
                        if let Some(m) = unix_mode {
                            files_by_unix_mode.add_mode(outpath, m);
                        } else {
                            files_by_unix_mode.clear_mode(&outpath);
                        }
                    }
                    #[cfg(any(unix, windows))]
                    SymlinkExtractAction::ExtractRecursiveInFolder
                    | SymlinkExtractAction::ExtractInFolder => {
                        if regular_files.contains_key(&dedup_key) {
                            return Err(ZipError::Io(std::io::Error::new(
                                std::io::ErrorKind::AlreadyExists,
                                "Cannot overwrite an extracted file with a symbolic link",
                            )));
                        }
                        make_symlink(
                            directory.as_ref(),
                            &outpath,
                            &target,
                            &self.shared.files,
                            symlink_action,
                        )?;
                    }
                    #[cfg(any(unix, windows))]
                    SymlinkExtractAction::ExtractNoRestrictions => {
                        if regular_files.contains_key(&dedup_key) {
                            return Err(ZipError::Io(std::io::Error::new(
                                std::io::ErrorKind::AlreadyExists,
                                "Cannot overwrite an extracted file with a symbolic link",
                            )));
                        }
                        let Ok(target_str) = std::str::from_utf8(&target) else {
                            return Err(invalid!("Invalid UTF-8 as symlink target"));
                        };
                        make_symlink_impl(
                            &outpath,
                            target_str,
                            &self.shared.files,
                            Some(directory.as_ref()),
                        )?;
                    }
                }
            } else if data.is_dir(file_name_raw) {
                if created_dirs.insert(outpath.clone()) {
                    make_writable_dir_all(&outpath, Some(directory.as_ref()))?;
                }
                if outpath != directory
                    && let Some(mode) = data.unix_mode()
                {
                    files_by_unix_mode.add_mode(outpath, mode);
                }
            } else {
                // Fold by `normalized_path_key` so Windows case-aliasing entries keep only
                // the later entry (matching serial extraction) and avoid parallel races.
                regular_files.insert(normalized_path_key(&outpath), (outpath, i));
            }
        }

        let metadata = self.metadata();

        let regular_files_vec: Vec<(std::path::PathBuf, usize)> =
            regular_files.into_values().collect();

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
                            .enforce_header_consistency(enforce_header_consistency);
                        let mut file = archive.by_index_with_options(idx, read_options)?;

                        let writable_mode = file.unix_mode().map(|_| 0o600);
                        let mut outfile =
                            create_outfile(&outpath, writable_mode, Some(directory.as_ref()))?;
                        let uncompressed_len = file.size();
                        let mut preallocated_len = 0u64;

                        if uncompressed_len > 0 && !file.flags().is_using_data_descriptor() {
                            let alloc_len = uncompressed_len.min(MAX_PREALLOCATE_LEN);
                            if outfile.set_len(alloc_len).is_ok() {
                                preallocated_len = alloc_len;
                            }
                        }

                        let copy_res = copy_into(&mut file, &mut outfile, buf);
                        if preallocated_len > 0 {
                            let actual_len = match &copy_res {
                                Ok(extracted) => *extracted,
                                Err(_) => outfile.stream_position().unwrap_or(0),
                            };
                            if actual_len < preallocated_len {
                                let _ = outfile.set_len(actual_len);
                            }
                        }
                        if let Err(e) = copy_res {
                            cleanup_partial_outfile_beneath(directory.as_ref(), &outpath, outfile);
                            return Err(e.into());
                        }

                        #[cfg(feature = "chrono")]
                        if let Some(last_modified) = file.last_modified()
                            && let Some(t) = last_modified.datetime_to_systemtime()
                        {
                            let _ = outfile.set_modified(t);
                        }

                        Ok((outpath, file.unix_mode()))
                    },
                )
                .collect();

        let extracted_files = regular_file_results?;

        for (path, mode) in extracted_files {
            if let Some(m) = mode {
                files_by_unix_mode.add_mode(path, m);
            } else {
                files_by_unix_mode.clear_mode(&path);
            }
        }

        #[cfg(unix)]
        for (path, perms) in files_by_unix_mode.all_perms_with_children_first() {
            set_permissions_no_follow_symlink(&path, perms, Some(directory.as_ref()))?;
        }
        #[cfg(not(unix))]
        for path in files_by_unix_mode.all_read_only_with_children_first() {
            set_readonly_beneath(directory.as_ref(), &path)?;
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
    ///
    /// Root-directory unwrapping can be enabled with [`ExtractOptions::with_root_dir_filter`].
    pub fn par_extract_from_path_with_options<P: AsRef<Path>, Q: AsRef<Path>>(
        &mut self,
        archive_path: P,
        destination: Q,
        options: ExtractOptions<impl RootDirFilter>,
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
    ///
    /// Root-directory unwrapping can be enabled with [`ExtractOptions::with_root_dir_filter`].
    pub fn par_extract_with_options<P: AsRef<Path>>(
        &mut self,
        directory: P,
        options: ExtractOptions<impl RootDirFilter>,
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

#[cfg(all(test, windows, not(miri)))]
mod tests {
    use super::{
        create_outfile, open_dir_beneath_windows, remove_outfile_beneath, set_readonly_beneath,
    };
    use std::io::{ErrorKind, Write};
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_WRITE_DATA,
    };

    #[test]
    fn windows_held_ancestor_blocks_write_and_delete_while_allowing_child_create() {
        let root = tempfile::TempDir::with_prefix("win_pin").unwrap();
        let dest = root.path().canonicalize().unwrap();
        let empty_dir = dest.join("empty_sub");

        let _pinned = open_dir_beneath_windows(&dest, &empty_dir, true).unwrap();

        let write_attempt = std::fs::OpenOptions::new()
            .access_mode(FILE_WRITE_DATA)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&empty_dir);
        assert!(
            matches!(write_attempt, Err(ref e) if e.kind() == ErrorKind::PermissionDenied || e.raw_os_error() == Some(32)),
            "expected sharing violation on FILE_WRITE_DATA for held empty directory, got {write_attempt:?}"
        );

        let delete_attempt = std::fs::OpenOptions::new()
            .access_mode(DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&empty_dir);
        assert!(
            matches!(delete_attempt, Err(ref e) if e.kind() == ErrorKind::PermissionDenied || e.raw_os_error() == Some(32)),
            "expected sharing violation on DELETE for held empty directory, got {delete_attempt:?}"
        );

        // While held, creating child files inside the pinned directory must still work
        // (both for serial extraction and parallel worker threads opening the same dir),
        // while concurrent write/delete/reparse attempts on the directory are rejected.
        let child = empty_dir.join("child.txt");
        let file = create_outfile(&child, None, Some(&dest)).unwrap();
        drop(file);
        assert!(child.is_file());

        drop(_pinned);
    }

    #[test]
    fn windows_remove_outfile_beneath_deletes_by_handle_and_set_readonly_skips_dest() {
        let root = tempfile::TempDir::with_prefix("win_rm_ro").unwrap();
        let dest = root.path().canonicalize().unwrap();
        let sub_file = dest.join("sub").join("partial.bin");

        let mut f = create_outfile(&sub_file, None, Some(&dest)).unwrap();
        f.write_all(b"partial").unwrap();
        remove_outfile_beneath(&dest, &sub_file, f).unwrap();
        assert!(!sub_file.exists());

        // `set_readonly_beneath` must never alter `dest` itself.
        set_readonly_beneath(&dest, &dest).unwrap();
        assert!(!dest.metadata().unwrap().permissions().readonly());

        // Regular files inside `dest` get the read-only attribute cleanly.
        let keep = dest.join("sub").join("keep.txt");
        let f2 = create_outfile(&keep, None, Some(&dest)).unwrap();
        drop(f2);
        set_readonly_beneath(&dest, &keep).unwrap();
        assert!(keep.metadata().unwrap().permissions().readonly());

        let mut perms = keep.metadata().unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        // Clean up read-only attribute before `TempDir` removal on Windows.
        perms.set_readonly(false);
        std::fs::set_permissions(&keep, perms).unwrap();

        // Pre-existing NTFS junction at the exact leaf passed to `create_outfile` must be
        // removed as a reparse point without modifying the target directory.
        let junction_target = dest.join("jtarget");
        std::fs::create_dir(&junction_target).unwrap();
        let sentinel = junction_target.join("sentinel.txt");
        std::fs::write(&sentinel, b"SAFE").unwrap();
        let junction = dest.join("jleaf");
        let status = std::process::Command::new("cmd.exe")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&junction)
            .arg(&junction_target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        if status.success() {
            let mut jf = create_outfile(&junction, None, Some(&dest)).unwrap();
            jf.write_all(b"REPLACED").unwrap();
            drop(jf);
            assert_eq!(std::fs::read(&junction).unwrap(), b"REPLACED");
            assert_eq!(std::fs::read(&sentinel).unwrap(), b"SAFE");
        }
    }

    #[test]
    fn windows_reparse_tag_allowlist_and_bypass_blocks() {
        use super::{is_disallowed_reparse_handle, is_disallowed_reparse_tag};
        use std::os::windows::fs::MetadataExt;
        use std::os::windows::io::AsRawHandle;

        // 1. Name-surrogate tags (bit 29 == 1) must always be blocked:
        for tag in [
            0xA000_000C, // IO_REPARSE_TAG_SYMLINK
            0xA000_0003, // IO_REPARSE_TAG_MOUNT_POINT / junction
            0xA000_001D, // IO_REPARSE_TAG_LX_SYMLINK
            0xA000_0019, // IO_REPARSE_TAG_GLOBAL_REPARSE
            0xA000_001F, // IO_REPARSE_TAG_WCI_TOMBSTONE
            0xA000_0022, // IO_REPARSE_TAG_PROJFS_TOMBSTONE
            0xA000_0027, // IO_REPARSE_TAG_WCI_LINK
            0xA000_1027, // IO_REPARSE_TAG_WCI_LINK_1
            0xA000_0028, // IO_REPARSE_TAG_DATALESS_CIM
        ] {
            assert!(
                is_disallowed_reparse_tag(tag),
                "surrogate tag {tag:#x} must be rejected"
            );
        }

        // 2. Allowlisted Microsoft non-redirecting cloud/storage/tiering filters must pass:
        for nibble in 0..=0xFu32 {
            let cloud_tag = 0x9000_001A | (nibble << 12); // IO_REPARSE_TAG_CLOUD..CLOUD_F
            assert!(
                !is_disallowed_reparse_tag(cloud_tag),
                "Cloud Files tag {cloud_tag:#x} should be allowed"
            );
        }
        for allowed in [
            0x8000_0021, // IO_REPARSE_TAG_ONEDRIVE
            0x8000_0015, // IO_REPARSE_TAG_FILE_PLACEHOLDER
            0x8000_0017, // IO_REPARSE_TAG_WOF
            0x8000_0013, // IO_REPARSE_TAG_DEDUP
            0x8000_001E, // IO_REPARSE_TAG_STORAGE_SYNC
            0x9000_0027, // IO_REPARSE_TAG_STORAGE_SYNC_FOLDER
            0x8000_0006, // IO_REPARSE_TAG_HSM2
            0xC000_0004, // IO_REPARSE_TAG_HSM
            0x8000_0007, // IO_REPARSE_TAG_SIS
            0xC000_0014, // IO_REPARSE_TAG_APPXSTRM
        ] {
            assert!(
                !is_disallowed_reparse_tag(allowed),
                "allowlisted tag {allowed:#x} should be allowed"
            );
        }

        // 3. Dangerous, redirecting, or unknown non-surrogate tags (bit 29 == 0) must be blocked:
        for blocked in [
            0x8000_001B, // IO_REPARSE_TAG_APPEXECLINK
            0x8000_000A, // IO_REPARSE_TAG_DFS
            0x8000_0012, // IO_REPARSE_TAG_DFSR
            0x8000_0014, // IO_REPARSE_TAG_NFS
            0x8000_0023, // IO_REPARSE_TAG_AF_UNIX
            0x8000_0024, // IO_REPARSE_TAG_LX_FIFO
            0x8000_0025, // IO_REPARSE_TAG_LX_CHR
            0x8000_0026, // IO_REPARSE_TAG_LX_BLK
            0x8000_0008, // IO_REPARSE_TAG_WIM
            0x8000_0009, // IO_REPARSE_TAG_CSV
            0x8000_0018, // IO_REPARSE_TAG_WCI
            0x9000_1018, // IO_REPARSE_TAG_WCI_1
            0x9000_001C, // IO_REPARSE_TAG_PROJFS
            0x9000_011A, // bit-toggled near CLOUD tag
            0x9001_001A, // bit-toggled near CLOUD tag
            0x0000_0042, // custom 3rd-party tag
            0x0000_0000, // zero tag
        ] {
            assert!(
                is_disallowed_reparse_tag(blocked),
                "non-allowlisted tag {blocked:#x} bypassed check!"
            );
        }

        let root = tempfile::TempDir::with_prefix("win_reparse_tag").unwrap();
        let dir = root.path().canonicalize().unwrap();
        let target = dir.join("target");
        let junction = dir.join("junction");
        std::fs::create_dir(&target).unwrap();

        let dir_handle = std::fs::OpenOptions::new()
            .access_mode(windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&target)
            .unwrap();
        let dir_attrs = target.metadata().unwrap().file_attributes();
        assert!(!is_disallowed_reparse_handle(dir_handle.as_raw_handle(), dir_attrs).unwrap());

        let status = std::process::Command::new("cmd.exe")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&junction)
            .arg(&target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        if status.success() {
            let j_handle = std::fs::OpenOptions::new()
                .access_mode(windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&junction)
                .unwrap();
            let j_attrs = std::fs::symlink_metadata(&junction)
                .unwrap()
                .file_attributes();
            assert!(is_disallowed_reparse_handle(j_handle.as_raw_handle(), j_attrs).unwrap());
        }

        // 4. Real WOF-compressed file via `compact.exe` stays writable via `create_outfile`.
        let wof_file = target.join("wof.bin");
        std::fs::write(&wof_file, vec![b'A'; 0x0001_0000]).unwrap();
        let compact_ok = std::process::Command::new("compact.exe")
            .arg("/c")
            .arg("/exe:xpress4k")
            .arg(&wof_file)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if compact_ok {
            let mut out = create_outfile(&wof_file, None, Some(&dir)).unwrap();
            out.write_all(b"OVERWROTE_WOF").unwrap();
            drop(out);
            assert_eq!(std::fs::read(&wof_file).unwrap(), b"OVERWROTE_WOF");
        }

        // 5. External reader holding FILE_SHARE_READ | FILE_SHARE_WRITE (omitting
        //    FILE_SHARE_DELETE) must not trigger a false-positive ERROR_SHARING_VIOLATION.
        let shared_file = target.join("shared.txt");
        std::fs::write(&shared_file, b"OLD").unwrap();
        let reader = std::fs::OpenOptions::new()
            .access_mode(windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES)
            .share_mode(
                windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ
                    | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE,
            )
            .open(&shared_file)
            .unwrap();
        let mut out = create_outfile(&shared_file, None, Some(&dir)).unwrap();
        out.write_all(b"UPDATED").unwrap();
        drop(out);
        drop(reader);
        assert_eq!(std::fs::read(&shared_file).unwrap(), b"UPDATED");

        // 6. `remove_outfile_beneath` cleans up handles opened without DELETE access.
        let no_del_file = target.join("no_del.txt");
        let ro_handle = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .access_mode(windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_WRITE)
            .share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ)
            .open(&no_del_file)
            .unwrap();
        remove_outfile_beneath(&dir, &no_del_file, ro_handle).unwrap();
        assert!(!no_del_file.exists());
    }

    #[test]
    fn windows_case_key_preserves_non_1_to_1_ntfs_names() {
        use super::windows_case_key;
        use std::path::Path;

        // Standard 1:1 ASCII case folding still matches.
        assert_eq!(
            windows_case_key(Path::new(r"C:\dest\Foo.txt")),
            windows_case_key(Path::new(r"C:\dest\foo.txt"))
        );
        // Multi-char Unicode expansions (`ß` -> `SS`, `İ` -> `I\u{307}`) represent
        // distinct files on NTFS and must not fold together.
        assert_ne!(
            windows_case_key(Path::new(r"C:\dest\straße.txt")),
            windows_case_key(Path::new(r"C:\dest\STRASSE.txt"))
        );
        assert_ne!(
            windows_case_key(Path::new(r"C:\dest\İ.txt")),
            windows_case_key(Path::new(r"C:\dest\i\u{307}.txt"))
        );
    }
}

#[cfg(all(test, unix, not(miri)))]
mod unix_tests {
    use super::{create_outfile, cstr, remove_outfile_beneath, set_permissions_no_follow_symlink};
    use std::io::{ErrorKind, Write};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn unix_0o000_permissions_and_dest_guard_do_not_follow_symlinks() {
        let root = tempfile::TempDir::with_prefix("unix_perm").unwrap();
        let dest = root.path().join("dest");
        let canary = root.path().join("canary.txt");
        std::fs::create_dir(&dest).unwrap();
        let dest = dest.canonicalize().unwrap();
        std::fs::write(&canary, b"keep").unwrap();
        std::fs::set_permissions(&canary, std::fs::Permissions::from_mode(0o644)).unwrap();

        let target = dest.join("secret.bin");
        let mut f = create_outfile(&target, Some(0o600), Some(&dest)).unwrap();
        f.write_all(b"x").unwrap();
        drop(f);

        // Apply `0o000` (unreadable & unwritable to owner), then restore `0o640` via pinned fd.
        set_permissions_no_follow_symlink(
            &target,
            std::fs::Permissions::from_mode(0o000),
            Some(&dest),
        )
        .unwrap();
        assert_eq!(
            target.metadata().unwrap().permissions().mode() & 0o777,
            0o000
        );

        set_permissions_no_follow_symlink(
            &target,
            std::fs::Permissions::from_mode(0o640),
            Some(&dest),
        )
        .unwrap();
        assert_eq!(
            target.metadata().unwrap().permissions().mode() & 0o777,
            0o640
        );

        // `path == dest` must be a no-op so `"./"` never chmods caller's root destination.
        let before_mode = dest.metadata().unwrap().permissions().mode() & 0o777;
        set_permissions_no_follow_symlink(
            &dest,
            std::fs::Permissions::from_mode(0o000),
            Some(&dest),
        )
        .unwrap();
        assert_eq!(
            dest.metadata().unwrap().permissions().mode() & 0o777,
            before_mode
        );

        // Symlink inside `dest` must not propagate permission changes to `canary`.
        let link = dest.join("link");
        std::os::unix::fs::symlink(&canary, &link).unwrap();
        set_permissions_no_follow_symlink(
            &link,
            std::fs::Permissions::from_mode(0o000),
            Some(&dest),
        )
        .unwrap();
        assert_eq!(
            canary.metadata().unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn unix_create_outfile_rejects_fifo_and_remove_outfile_beneath_rejects_symlink_swap() {
        let root = tempfile::TempDir::with_prefix("unix_fifo_rm").unwrap();
        let dest = root.path().canonicalize().unwrap();
        let fifo = dest.join("fifo");
        let fifo_c = cstr(fifo.as_os_str()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);

        // Must reject immediately without blocking on FIFO open.
        let res = create_outfile(&fifo, Some(0o600), Some(&dest));
        assert!(res.is_err(), "FIFO open must not succeed or block");

        let canary = root.path().join("canary_victim.txt");
        std::fs::write(&canary, b"keep").unwrap();
        let file_path = dest.join("swapped.txt");
        let mut outfile = create_outfile(&file_path, Some(0o600), Some(&dest)).unwrap();
        outfile.write_all(b"partial").unwrap();

        // Simulate concurrent unlink + symlink swap of `swapped.txt` while `outfile` is open.
        std::fs::remove_file(&file_path).unwrap();
        std::os::unix::fs::symlink(&canary, &file_path).unwrap();

        let err = remove_outfile_beneath(&dest, &file_path, outfile).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(
            canary.exists(),
            "swapped symlink target must never be unlinked"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn unix_open_dir_beneath_traverses_search_only_dirs() {
        let root = tempfile::TempDir::with_prefix("unix_search_only").unwrap();
        let dest = root.path().canonicalize().unwrap();
        let sub = dest.join("search_only");
        std::fs::create_dir(&sub).unwrap();
        let file = sub.join("f.txt");
        let f = create_outfile(&file, Some(0o600), Some(&dest)).unwrap();
        drop(f);

        // Strip read and write permission bits (mode 0o100: execute/search only).
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o100)).unwrap();

        // Traversing without `create_and_make_writable` must succeed via O_PATH (+x only).
        let _dirfd = super::open_dir_beneath(&dest, &sub, false).unwrap();

        // Creating a sibling file inside the search-only directory with `create_and_make_writable=true`
        // must also lift permissions via `/proc/self/fd` without failing EACCES on O_RDONLY.
        let file2 = sub.join("f2.txt");
        let mut f2 = create_outfile(&file2, Some(0o600), Some(&dest)).unwrap();
        f2.write_all(b"ok").unwrap();
        drop(f2);
        assert_eq!(std::fs::read(&file2).unwrap(), b"ok");
    }
}
