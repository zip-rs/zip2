//! Stream-oriented filesystem extraction.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use crate::fs::set_permissions_no_follow_symlink;
use crate::fs::{
    EXTRACT_BUFFER_SIZE, MAX_SYMLINK_TARGET_LEN, copy_into, create_outfile, make_symlink,
    make_symlink_as_file, make_symlink_as_file_utf8,
};
use crate::path::ZipFilePathExt;
use indexmap::IndexMap;
use zip_core_lib::read::{ExtractOptions, SymlinkExtractAction};
use zip_core_lib::result::{ZipResult, invalid};
use zip_read::read::stream::{ZipStreamReader, ZipStreamVisitor};
use zip_read::read::{ZipFile, ZipFileEntry};

pub trait ZipStreamExtract {
    /// Extracts a streaming ZIP reader into a directory.
    fn extract<P: AsRef<Path>>(self, directory: P) -> ZipResult<()>;
}

impl<R: Read> ZipStreamExtract for ZipStreamReader<R> {
    /// Extract a Zip archive into a directory, overwriting files if they
    /// already exist. Paths are sanitized with [`ZipFilePathExt::enclosed_name`].
    ///
    /// Extraction is not atomic; If an error is encountered, some of the files
    /// may be left on disk. The file(s) and dir(s) are first created, then the
    /// permissions are applied to them
    ///
    /// Extraction of symlink is not possible since we don't have access to the
    /// external attributes in the local headers of the entries
    fn extract<P: AsRef<Path>>(self, directory: P) -> ZipResult<()> {
        /// Destination, every entry written in this pass (raw local name -> the checked output
        /// path `visit_file` wrote it to), and the modes the central directory gives them.
        struct Extractor {
            outpath: PathBuf,
            resolved_paths: IndexMap<Box<[u8]>, PathBuf>,
            file_permissions: std::collections::BTreeMap<PathBuf, u32>,
            buf: Vec<u8>,
        }
        impl ZipStreamVisitor for Extractor {
            fn visit_file<R: Read>(&mut self, file: &mut ZipFile<'_, R>) -> ZipResult<()> {
                let extract_options = ExtractOptions::default();
                let outpath =
                    file.safe_prepare_path(&self.outpath, None::<&(_, fn(&Path) -> bool)>)?;
                self.resolved_paths
                    .insert(file.name_raw().into(), outpath.clone());

                if file.is_symlink() {
                    // Same bound as `ZipArchive::extract`: the declared
                    // uncompressed size is attacker-controlled, so reject
                    // oversized entries rather than pre-allocating them.
                    //
                    // `is_symlink()` is currently always false on this path
                    // (`from_local_block` has no external attributes to read),
                    // so this is defence in depth -- see
                    // `stream_does_not_expose_the_symlink_bit`.
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
                            make_symlink_as_file(&outpath, &target, file.unix_mode())?;
                        }
                        SymlinkExtractAction::ExtractAsFileUtf8 => {
                            make_symlink_as_file_utf8(&outpath, &target, file.unix_mode())?;
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
                    fs::create_dir_all(&outpath)?;
                } else {
                    let mut outfile = create_outfile(&outpath, file.unix_mode())?;
                    copy_into(file, &mut outfile, &mut self.buf)?;
                }

                Ok(())
            }

            fn visit_additional_metadata(&mut self, metadata: &ZipFileEntry<'_>) -> ZipResult<()> {
                // The central directory is not checked against the local headers, so its
                // names only select entries this pass already wrote: a name that wasn't
                // written gets no mode, and the mode goes to the path `visit_file` checked,
                // never to a path rebuilt from the central directory.
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
            // An entry named like `staging` would be moved into it, so first rename `staging`
            // to a name no entry has. Ask the filesystem, since it may ignore case.
            let written_to = staging.clone();
            let has_entry_named_like = |dir: &Path| {
                dir.file_name()
                    .is_some_and(|name| dir.join(name).symlink_metadata().is_ok())
            };
            while has_entry_named_like(staging) {
                let renamed = staging_path(dest)?;
                fs::rename(&*staging, &renamed)?;
                *staging = renamed;
            }

            for staged in extractor.resolved_paths.values_mut() {
                let relative = staged
                    .strip_prefix(&written_to)
                    .map_err(|_| invalid!("Entry was not extracted to the staging directory"))?;
                let from = staging.join(relative);
                let outpath = crate::path::resolve_enclosed(dest, dest, relative.iter(), true)?;
                if outpath.starts_with(&*staging) {
                    return Err(invalid!("Entry would be written to the staging directory"));
                }
                if fs::symlink_metadata(&from)?.is_dir() {
                    fs::create_dir_all(&outpath)?;
                } else if let Err(e) = fs::rename(&from, &outpath) {
                    if e.kind() != io::ErrorKind::CrossesDevices {
                        return Err(e.into());
                    }
                    fs::copy(&from, &outpath)?;
                }
                if let Some(mode) = extractor.file_permissions.remove(&*staged) {
                    extractor.file_permissions.insert(outpath.clone(), mode);
                }
                *staged = outpath;
            }
            Ok(())
        }

        use std::fs;
        fs::create_dir_all(&directory)?;
        let directory = directory.as_ref().canonicalize()?;

        // The headers are only checked after the last entry, so write nothing to `directory`
        // before that.
        let staging = if self.header_consistency_enforced() {
            Some(create_staging_dir(&directory)?)
        } else {
            None
        };

        let mut extractor = Extractor {
            outpath: staging.clone().unwrap_or_else(|| directory.clone()),
            resolved_paths: IndexMap::new(),
            file_permissions: Default::default(),
            buf: vec![0u8; EXTRACT_BUFFER_SIZE],
        };
        let visited = self.visit(&mut extractor);
        if let Some(mut staging) = staging {
            let moved = visited.and_then(|()| {
                let mut staged_paths = std::collections::HashSet::new();
                let mut staged_files = std::collections::HashSet::new();
                let mut output_paths = std::collections::HashSet::new();
                for staged in extractor.resolved_paths.values() {
                    let relative = staged.strip_prefix(&staging).map_err(|_| {
                        invalid!("Entry was not extracted to the staging directory")
                    })?;
                    let output = crate::path::resolve_enclosed(
                        &directory,
                        &directory,
                        relative.iter(),
                        false,
                    )?;
                    if !staged_paths.insert(staged)
                        || !output_paths.insert(output)
                        || staged_file_identity(staged)?
                            .is_some_and(|identity| !staged_files.insert(identity))
                    {
                        return Err(invalid!("Multiple entries resolve to the same output path"));
                    }
                }
                move_staged_entries(&mut staging, &directory, &mut extractor)
            });
            let removed = fs::remove_dir_all(&staging);
            moved?;
            removed?;
        } else {
            visited?;
        }

        // As `ZipArchive::extract`: children before parents, so a read-only directory does not
        // block setting its contents. Never through a symlink: `set_permissions` follows them.
        // Off Unix, only the read-only flag applies.
        for (path, mode) in extractor.file_permissions.into_iter().rev() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                set_permissions_no_follow_symlink(&path, std::fs::Permissions::from_mode(mode))?;
            }
            #[cfg(not(unix))]
            if mode & 0o222 == 0 {
                let metadata = fs::symlink_metadata(&path)?;
                if !metadata.is_symlink() {
                    let mut permissions = metadata.permissions();
                    permissions.set_readonly(true);
                    fs::set_permissions(&path, permissions)?;
                }
            }
        }
        Ok(())
    }
}

/// Filesystem identity catches aliases on case-insensitive filesystems even when the
/// archive names and their checked path strings differ.
fn staged_file_identity(
    #[cfg_attr(not(any(unix, windows)), allow(unused_variables))] path: &Path,
) -> io::Result<Option<(u64, u64)>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path)?;
        Ok(Some((metadata.dev(), metadata.ino())))
    }
    #[cfg(windows)]
    {
        use std::fs::OpenOptions;
        use std::io;
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };

        let file = OpenOptions::new()
            .access_mode(0) // 0 means no read/write data access needed, just metadata
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        let handle = file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
        // Safety: We provide a valid file handle and a mutable pointer to a zeroed struct.
        unsafe {
            let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
            let result = GetFileInformationByHandle(handle, &mut info);

            if result == 0 {
                return Err(io::Error::last_os_error());
            }

            let volume_serial_number = u64::from(info.dwVolumeSerialNumber);
            // Combine the high and low 32-bit fields into a single 64-bit file index
            let file_index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);

            Ok(Some((volume_serial_number, file_index)))
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(None)
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
        #[cfg(unix)]
        let created = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(&staging)
        };
        #[cfg(windows)]
        let created = create_private_staging_dir_windows(&staging);
        #[cfg(not(any(unix, windows)))]
        let created = std::fs::create_dir(&staging);
        match created {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            result => return result.map(|()| staging),
        }
    }
}

#[cfg(windows)]
fn create_private_staging_dir_windows(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;

    // Protect the new directory and its children from inheriting a shared parent's ACL.
    // Owner Rights, SYSTEM and Administrators can access staged files. The ACL is set
    // during creation so there is no period when another user can read staged data.
    let sddl: Vec<u16> = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: The SDDL string is NUL-terminated and descriptor receives an owned allocation.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: Both the path and security descriptor remain live for this call.
    let created = unsafe { CreateDirectoryW(path.as_ptr(), &attributes) };
    let result = if created == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    };
    // SAFETY: ConvertStringSecurityDescriptorToSecurityDescriptorW used LocalAlloc.
    unsafe { LocalFree(descriptor) };
    result
}
