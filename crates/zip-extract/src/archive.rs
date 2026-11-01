//! Filesystem extraction extensions for shared ZIP archive reader types.

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use crate::fs::{
    MAX_SYMLINK_TARGET_LEN, make_symlink, make_symlink_as_file, make_symlink_as_file_utf8,
};
use indexmap::IndexMap;
pub use zip_core_lib::read::{
    ExtractOptions, RootDirFilter, SymlinkExtractAction, root_dir_common_filter,
};
use zip_core_lib::result::{ZipError, ZipResult, invalid};

use crate::path::ZipFileDataPathExt;
use zip_core_lib::{ZipReadOptions, types::ZipFileData};
pub use zip_read::read::{ZipArchive, ZipFile, ZipFileEntry};

/// Filesystem extraction operations for [`ZipArchive`].
pub trait ZipExtract {
    /// Finds the archive's single common top-level directory, if present.
    fn root_dir(&self, filter: impl RootDirFilter) -> ZipResult<Option<PathBuf>>;

    /// Extracts the archive to a destination directory.
    fn extract<P: AsRef<Path>>(&mut self, destination: P) -> ZipResult<()>;

    /// Extracts with symlink, size-limit, and header-validation options.
    fn extract_with_options<P: AsRef<Path>>(
        &mut self,
        destination: P,
        options: ExtractOptions,
    ) -> ZipResult<()>;

    /// Extracts after removing the archive's single common top-level directory when present.
    fn extract_unwrapped_root_dir<P: AsRef<Path>, F: RootDirFilter>(
        &mut self,
        destination: P,
        filter: F,
    ) -> ZipResult<()>;

    /// Parallel-extraction-compatible entry point; falls back to safe sequential extraction.
    fn par_extract<P: AsRef<Path>>(&mut self, destination: P) -> ZipResult<()>;

    /// Parallel-extraction-compatible entry point with extraction options.
    fn par_extract_with_options<P: AsRef<Path>>(
        &mut self,
        destination: P,
        options: ExtractOptions,
    ) -> ZipResult<()>;

    /// Extracts an archive file from disk to a destination directory.
    fn par_extract_from_path<P: AsRef<Path>, Q: AsRef<Path>>(
        &mut self,
        archive_path: P,
        destination: Q,
    ) -> ZipResult<()>;
}

impl<R: Read + Seek> ZipExtract for ZipArchive<R> {
    fn root_dir(&self, filter: impl RootDirFilter) -> ZipResult<Option<PathBuf>> {
        let mut root_dir: Option<PathBuf> = None;
        for index in 0..self.len() {
            let file = self.by_index_data(index)?;
            let data = file.data.as_ref();
            let file_name = match data.name(&file.file_name_raw) {
                Ok(file_name) => file_name,
                Err(_) => continue,
            };
            let path = match data.enclosed_name(&file_name) {
                Some(path) => path,
                None => return Ok(None),
            };

            if !filter(&path) {
                continue;
            }

            if path.components().count() == 1 {
                if data.is_dir(&file.file_name_raw) {
                    if let Some(root) = &root_dir {
                        if *root != path {
                            return Ok(None);
                        }
                    } else {
                        root_dir = Some(path);
                    }
                    continue;
                }
                return Ok(None);
            }

            let mut path = path.as_path();
            while let Some(parent) = path.parent().filter(|path| *path != Path::new("")) {
                path = parent;
            }
            if let Some(root) = &root_dir {
                if root != path {
                    return Ok(None);
                }
            } else {
                root_dir = Some(path.into());
            }
        }
        Ok(root_dir)
    }

    fn extract<P: AsRef<Path>>(&mut self, destination: P) -> ZipResult<()> {
        self.extract_with_options(destination, ExtractOptions::default())
    }

    fn extract_with_options<P: AsRef<Path>>(
        &mut self,
        destination: P,
        options: ExtractOptions,
    ) -> ZipResult<()> {
        extract_entries(self, destination.as_ref(), options, None)
    }

    fn extract_unwrapped_root_dir<P: AsRef<Path>, F: RootDirFilter>(
        &mut self,
        destination: P,
        filter: F,
    ) -> ZipResult<()> {
        let root = self.root_dir(filter)?;
        extract_entries(
            self,
            destination.as_ref(),
            ExtractOptions::default(),
            root.as_deref(),
        )
    }

    fn par_extract<P: AsRef<Path>>(&mut self, destination: P) -> ZipResult<()> {
        self.extract(destination)
    }

    fn par_extract_with_options<P: AsRef<Path>>(
        &mut self,
        destination: P,
        options: ExtractOptions,
    ) -> ZipResult<()> {
        self.extract_with_options(destination, options)
    }

    fn par_extract_from_path<P: AsRef<Path>, Q: AsRef<Path>>(
        &mut self,
        archive_path: P,
        destination: Q,
    ) -> ZipResult<()> {
        let mut archive = ZipArchive::new(fs::File::open(archive_path)?)?;
        archive.extract(destination)
    }
}

fn extract_entries<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    destination: &Path,
    options: ExtractOptions,
    strip_root: Option<&Path>,
) -> ZipResult<()> {
    if let Some(limit) = options.size_limit_value() {
        match archive.decompressed_size() {
            Some(size) if size > u128::from(limit) => {
                return Err(ZipError::DecompressedSizeLimitExceeded { size, limit });
            }
            Some(_) => {}
            None => return Err(ZipError::DecompressedSizeUnknown),
        }
    }
    fs::create_dir_all(destination)?;
    let base = fs::canonicalize(destination)?;
    let existing_files: IndexMap<Box<[u8]>, ZipFileData> = IndexMap::new();
    let mut directory_permissions = Vec::new();

    for index in 0..archive.len() {
        let read_options =
            ZipReadOptions::new().enforce_header_consistency(options.header_consistency_enforced());
        let mut file = archive.by_index_with_options(index, read_options)?;
        #[cfg(windows)]
        if has_invalid_ads_component(file.name_raw()) {
            return Err(invalid!(
                "Alternate data stream paths are not valid extraction targets"
            ));
        }
        let file_name = file.name()?;
        let components = crate::path::simplified_components(Path::new(file_name.as_ref()))
            .ok_or_else(|| invalid!("Invalid file path"))?;
        let mut relative = PathBuf::from_iter(components);
        if let Some(root) = strip_root {
            if let Ok(stripped) = relative.strip_prefix(root) {
                relative = stripped.to_path_buf();
                if relative.as_os_str().is_empty() {
                    continue;
                }
            }
        }

        validate_output_path(&relative)?;
        let output = base.join(relative);
        let is_symlink = file.is_symlink();
        if is_symlink
            && matches!(
                options.symlink_action_value(),
                SymlinkExtractAction::NoExtract
            )
        {
            continue;
        }

        if is_symlink {
            if file.size() > MAX_SYMLINK_TARGET_LEN {
                return Err(invalid!(
                    "symlink target declares {} bytes, more than the {} byte limit",
                    file.size(),
                    MAX_SYMLINK_TARGET_LEN
                ));
            }
            let parent = output
                .parent()
                .ok_or_else(|| invalid!("Invalid extraction path"))?;
            fs::create_dir_all(parent)?;
            validate_parent(&base, &output)?;
            if fs::symlink_metadata(&output).is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err(invalid!(
                    "Extraction would overwrite an existing symbolic link"
                ));
            }
            let mut target = Vec::new();
            file.read_to_end(&mut target)?;
            match options.symlink_action_value() {
                SymlinkExtractAction::NoExtract => continue,
                SymlinkExtractAction::ExtractAsFile => {
                    make_symlink_as_file(&output, &target, file.unix_mode())?;
                }
                SymlinkExtractAction::ExtractAsFileUtf8 => {
                    make_symlink_as_file_utf8(&output, &target, file.unix_mode())?;
                }
                #[cfg(any(unix, windows))]
                action => {
                    make_symlink(&base, &output, &target, &existing_files, action)?;
                }
            }
            continue;
        }

        if file.is_dir() {
            fs::create_dir_all(&output)?;
            validate_parent(&base, &output)?;
            directory_permissions.push((output, file.unix_mode()));
            continue;
        }

        let parent = output
            .parent()
            .ok_or_else(|| invalid!("Invalid extraction path"))?;
        fs::create_dir_all(parent)?;
        validate_parent(&base, &output)?;
        if fs::symlink_metadata(&output).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(invalid!(
                "Extraction would overwrite an existing symbolic link"
            ));
        }

        let mut output_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&output)?;
        std::io::copy(&mut file, &mut output_file)?;
        apply_file_mode(&output, file.unix_mode())?;
    }

    for (directory, mode) in directory_permissions.into_iter().rev() {
        apply_directory_mode(&directory, mode)?;
    }
    Ok(())
}

fn validate_parent(base: &Path, output: &Path) -> ZipResult<()> {
    let parent = output
        .parent()
        .ok_or_else(|| invalid!("Invalid extraction path"))?;
    let canonical_parent = fs::canonicalize(parent)?;
    if !canonical_parent.starts_with(base) {
        return Err(invalid!("Extraction path escapes destination"));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_output_path(output: &Path) -> ZipResult<()> {
    if has_invalid_ads_component(output.as_os_str().to_string_lossy().as_bytes()) {
        return Err(invalid!(
            "Alternate data stream paths are not valid extraction targets"
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn validate_output_path(_output: &Path) -> ZipResult<()> {
    Ok(())
}

#[cfg(windows)]
fn has_invalid_ads_component(path: &[u8]) -> bool {
    path.split(|byte| matches!(byte, b'/' | b'\\'))
        .any(|component| {
            let colons: Vec<_> = component
                .iter()
                .enumerate()
                .filter_map(|(index, byte)| (*byte == b':').then_some(index))
                .collect();
            !colons.is_empty()
                && !(colons.len() == 1 && colons[0] == 1 && component[0].is_ascii_alphabetic())
        })
}

#[cfg(unix)]
fn apply_file_mode(path: &Path, mode: Option<u32>) -> ZipResult<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn apply_file_mode(_path: &Path, _mode: Option<u32>) -> ZipResult<()> {
    Ok(())
}

#[cfg(unix)]
fn apply_directory_mode(path: &Path, mode: Option<u32>) -> ZipResult<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn apply_directory_mode(_path: &Path, _mode: Option<u32>) -> ZipResult<()> {
    Ok(())
}
