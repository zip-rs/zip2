//! Filesystem operations used by archive extraction.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::Path;

use indexmap::IndexMap;
use zip_core_lib::read::extract::SymlinkExtractAction;
use zip_core_lib::result::{ZipError, ZipResult, invalid};

/// Upper bound on the uncompressed size of a symlink entry.
///
/// A symlink target is a filesystem path, so its length is bounded by the
/// platform (`PATH_MAX` is 4096 on Linux) no matter what the archive says. The
/// uncompressed size stored in the central directory is attacker-controlled,
/// so entries above this bound are rejected rather than pre-allocated.
pub const MAX_SYMLINK_TARGET_LEN: u64 = 4096;

/// Buffer capacity for file extraction streams, reducing syscall overhead
/// and optimizing page cache utilization during archive extraction.
pub const EXTRACT_BUFFER_SIZE: usize = 256 * 1024;

/// Maximum extent pre-allocation ceiling (64 MiB).
///
/// Pre-allocating disk space via `File::set_len` before streaming uncompressed bytes
/// reduces filesystem fragmentation and optimizes NTFS/ext4 block allocation.
/// Capped at 64 MiB to prevent denial-of-service / zero-fill amplification attacks
/// where an archive header claims an exorbitantly large uncompressed size.
pub const MAX_PREALLOCATE_LEN: u64 = 64 * 1024 * 1024;

/// Copies decompressed data from `reader` into `writer` using `buf` as a reusable transfer buffer.
pub fn copy_into<R: Read + ?Sized, W: Write + ?Sized>(
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

pub fn make_writable_dir_all<T: AsRef<Path>>(outpath: T) -> Result<(), ZipError> {
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
pub fn make_symlink_impl<T>(
    outpath: &Path,
    target_str: &str,
    _existing_files: &IndexMap<Box<[u8]>, T>,
) -> ZipResult<()> {
    std::os::unix::fs::symlink(Path::new(&target_str), outpath)?;
    Ok(())
}

#[cfg(windows)]
pub fn make_symlink_impl<T>(
    outpath: &Path,
    target_str: &str,
    existing_files: &IndexMap<Box<[u8]>, T>,
) -> ZipResult<()> {
    use zip_core_lib::format::functions::is_dir;
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
pub fn make_symlink<T>(
    base: &Path, // canonicalized destination directory
    outpath: &Path,
    target: &[u8],
    #[cfg_attr(not(any(windows, unix)), allow(unused))] existing_files: &IndexMap<Box<[u8]>, T>,
    symlink_action: SymlinkExtractAction,
) -> ZipResult<()> {
    let Ok(target_str) = std::str::from_utf8(target) else {
        return Err(invalid!("Invalid UTF-8 as symlink target"));
    };

    if matches!(symlink_action, SymlinkExtractAction::ExtractNoRestrictions) {
        return make_symlink_impl(outpath, target_str, existing_files);
    }

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
pub fn make_symlink<T>(
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

pub fn make_symlink_as_file_utf8(
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
pub fn create_outfile(outpath: &Path, unix_mode: Option<u32>) -> std::io::Result<std::fs::File> {
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

pub fn make_symlink_as_file(
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
pub fn set_permissions_no_follow_symlink(
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
