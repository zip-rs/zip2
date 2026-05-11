//! Code related to zipfile

use crate::CompressionMethod;
use crate::DateTime;
use crate::read::ExtraField;
use crate::HasZipMetadata;
use crate::ZIP64_BYTES_THR;
use crate::read::RootDirFilter;
use crate::read::make_writable_dir_all;
use crate::read::readers::{ZipFileReader, ZipFileSeekReader};
use crate::result::ZipResult;
use crate::result::invalid;
use crate::types::ZipFileData;
use crate::types::{SimpleFileOptions, ffi};
use core::mem::replace;
use std::borrow::Cow;
use std::ffi::OsStr;
use std::io::{self, Read, Seek, SeekFrom, copy, sink};
use std::path::{Component, Path, PathBuf};

/// A struct for reading a zip file
///
/// When reading from a `ZipFile` using [`Self::read()`], keep in mind that `read()` **does not guarantee** the buffer will be fully filled in a single call.
///
/// If your logic depends on the buffer being completely populated, use [`Self::read_exact()`] instead. It will continue reading until the entire buffer is filled or an error occurs.
#[derive(Debug)]
pub struct ZipFile<'a, R: Read + ?Sized> {
    pub(crate) file_name_raw: Cow<'a, [u8]>,
    pub(crate) data: Cow<'a, ZipFileData>,
    pub(crate) reader: ZipFileReader<'a, R>,
}

/// A struct for reading and seeking a zip file
pub struct ZipFileSeek<'a, R> {
    pub(crate) data: Cow<'a, ZipFileData>,
    pub(crate) reader: ZipFileSeekReader<'a, R>,
}

/// Methods for retrieving information on zip files
impl<'a, R: Read + ?Sized> ZipFile<'a, R> {
    pub(crate) fn take_raw_reader(&mut self) -> io::Result<io::Take<&'a mut R>> {
        replace(&mut self.reader, ZipFileReader::NoReader).into_inner()
    }

    /// Get the version of the file
    pub fn version_made_by(&self) -> (u8, u8) {
        (
            self.get_metadata().version_made_by / 10,
            self.get_metadata().version_made_by % 10,
        )
    }

    /// Get the name of the file
    ///
    /// # Warnings
    ///
    /// It is dangerous to use this name directly when extracting an archive.
    /// It may contain an absolute path (`/etc/shadow`), or break out of the
    /// current directory (`../runtime`). Carelessly writing to these paths
    /// allows an attacker to craft a ZIP archive that will overwrite critical
    /// files.
    ///
    /// You can use the [`ZipFile::enclosed_name`] method to validate the name
    /// as a safe path.
    pub fn name(&self) -> ZipResult<Cow<'_, str>> {
        self.data.name(&self.file_name_raw)
    }

    /// Get the name of the file, in the raw (internal) byte representation.
    ///
    /// The encoding of this data is currently undefined.
    pub fn name_raw(&self) -> &[u8] {
        &self.file_name_raw
    }

    /// Rewrite the path, ignoring any path components with special meaning.
    ///
    /// - Absolute paths are made relative
    /// - [`ParentDir`]s are ignored
    /// - Truncates the filename at a NULL byte
    ///
    /// This is appropriate if you need to be able to extract *something* from
    /// any archive, but will easily misrepresent trivial paths like
    /// `foo/../bar` as `foo/bar` (instead of `bar`). Because of this,
    /// [`ZipFile::enclosed_name`] is the better option in most scenarios.
    ///
    /// [`ParentDir`]: `Component::ParentDir`
    pub fn mangled_name(&self) -> ZipResult<PathBuf> {
        let file_name = self.name()?;
        let sanitized = self.data.file_name_sanitized(&file_name);
        Ok(sanitized)
    }

    /// Ensure the file path is safe to use as a [`Path`].
    ///
    /// - It can't contain NULL bytes
    /// - It can't resolve to a path outside the current directory
    ///   > `foo/../bar` is fine, `foo/../../bar` is not.
    /// - It can't be an absolute path
    ///
    /// This will read well-formed ZIP files correctly, and is resistant
    /// to path-based exploits. It is recommended over
    /// [`ZipFile::mangled_name`].
    pub fn enclosed_name(&self) -> Option<PathBuf> {
        self.data.enclosed_name(&self.name().ok()?)
    }

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

                // In this case, we expect that the file was not in the root
                // directory, but was filtered out when searching for the
                // root directory.
                None => {
                    // We could technically find ourselves at this code
                    // path if the user provides an unstable or
                    // non-deterministic `filter` function.
                    //
                    // If debug assertions are on, we should panic here.
                    // Otherwise, the safest thing to do here is to just
                    // extract as-is.
                    debug_assert!(
                        !filter(&PathBuf::from_iter(components.iter())),
                        "Root directory filter should not match at this point"
                    );

                    // Extract as-is.
                    &components[..]
                }
            },

            None => &components[..],
        };

        let components_len = components.len();

        for (is_last, component) in components
            .iter()
            .enumerate()
            .map(|(i, c)| (i == components_len - 1, c))
        {
            // we can skip the target directory itself because the base path is assumed to be "trusted" (if the user say extract to a symlink we can follow it)
            outpath.push(component);

            // check if the path is a symlink, the target must be _inherently_ within the directory
            for limit in (0..5u8).rev() {
                let meta = match std::fs::symlink_metadata(&outpath) {
                    Ok(meta) => meta,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {
                        if !is_last {
                            make_writable_dir_all(&outpath)?;
                        }
                        break;
                    }
                    Err(e) => return Err(e.into()),
                };

                if !meta.is_symlink() {
                    break;
                }

                if limit == 0 {
                    return Err(invalid!("Extraction followed a symlink too deep"));
                }

                // note that we cannot accept links that do not inherently resolve to a path inside the directory to prevent:
                // - disclosure of unrelated path exists (no check for a path exist and then ../ out)
                // - issues with file-system specific path resolution (case sensitivity, etc)
                let target = std::fs::read_link(&outpath)?;

                if !crate::path::simplified_components(&target)
                    .ok_or(invalid!("Invalid symlink target path"))?
                    .starts_with(
                        &crate::path::simplified_components(base_path)
                            .ok_or(invalid!("Invalid base path"))?,
                    )
                {
                    let is_absolute_enclosed = base_path
                        .components()
                        .map(Some)
                        .chain(std::iter::once(None))
                        .zip(target.components().map(Some).chain(std::iter::repeat(None)))
                        .all(|(a, b)| match (a, b) {
                            // both components are normal
                            (Some(Component::Normal(a)), Some(Component::Normal(b))) => a == b,
                            // both components consumed fully
                            (None, None) => true,
                            // target consumed fully but base path is not
                            (Some(_), None) => false,
                            // base path consumed fully but target is not (and normal)
                            (None, Some(Component::CurDir | Component::Normal(_))) => true,
                            _ => false,
                        });

                    if !is_absolute_enclosed {
                        return Err(invalid!("Symlink is not inherently safe"));
                    }
                }

                outpath.push(target);
            }
        }
        Ok(())
    }

    /// Get the comment of the file
    pub fn comment(&self) -> &str {
        &self.get_metadata().file_comment
    }

    /// Get the compression method used to store the file
    pub fn compression(&self) -> CompressionMethod {
        self.get_metadata().compression_method
    }

    /// Get if the files is encrypted or not
    pub fn encrypted(&self) -> bool {
        self.data.is_encrypted()
    }

    /// Get the size of the file, in bytes, in the archive
    pub fn compressed_size(&self) -> u64 {
        self.get_metadata().compressed_size
    }

    /// Get the size of the file, in bytes, when uncompressed
    pub fn size(&self) -> u64 {
        self.get_metadata().uncompressed_size
    }

    /// Get the time the file was last modified
    pub fn last_modified(&self) -> Option<DateTime> {
        self.data.last_modified_time
    }
    /// Returns whether the file is actually a directory
    pub fn is_dir(&self) -> bool {
        self.data.is_dir(&self.file_name_raw)
    }

    /// Returns whether the file is actually a symbolic link
    pub fn is_symlink(&self) -> bool {
        self.unix_mode()
            .is_some_and(|mode| mode & ffi::S_IFLNK == ffi::S_IFLNK)
    }

    /// Returns whether the file is a normal file (i.e. not a directory or symlink)
    pub fn is_file(&self) -> bool {
        !self.is_dir() && !self.is_symlink()
    }

    /// Get unix mode for the file
    pub fn unix_mode(&self) -> Option<u32> {
        self.get_metadata().unix_mode()
    }

    /// Get the CRC32 hash of the original file
    pub fn crc32(&self) -> u32 {
        self.get_metadata().crc32
    }

    /// Get the extra data of the zip header for this file
    pub fn extra_data(&self) -> Option<&[u8]> {
        self.get_metadata().extra_field.as_deref()
    }

    /// Get the starting offset of the data of the compressed file
    pub fn data_start(&self) -> Option<u64> {
        self.data.data_start.get().copied()
    }

    /// Get the starting offset of the zip header for this file
    pub fn header_start(&self) -> u64 {
        self.get_metadata().header_start
    }
    /// Get the starting offset of the zip header in the central directory for this file
    pub fn central_header_start(&self) -> u64 {
        self.get_metadata().central_header_start
    }

    /// Get the [`SimpleFileOptions`] that would be used to write this file to
    /// a new zip archive.
    pub fn options(&self) -> SimpleFileOptions {
        let mut options = SimpleFileOptions::default()
            .large_file(self.compressed_size().max(self.size()) > ZIP64_BYTES_THR)
            .compression_method(self.compression())
            .unix_permissions(self.unix_mode().unwrap_or(0o644) | ffi::S_IFREG)
            .last_modified_time(
                self.last_modified()
                    .filter(DateTime::is_valid)
                    .unwrap_or_else(DateTime::default_for_write),
            );

        options.normalize();
        #[cfg(feature = "aes-crypto")]
        if let Some((mode, vendor_version)) = self.get_metadata().aes_mode {
            use crate::types::EncryptWith;
            // Preserve AES metadata in options for downstream writers.
            // This is metadata-only and does not trigger encryption.
            options.encrypt_with = Some(EncryptWith::Aes {
                mode,
                vendor_version,
                salt: None,
                password: None,
            });
        }
        options
    }
}

/// Methods for retrieving information on zip files
impl<R: Read> ZipFile<'_, R> {
    /// iterate through all extra fields
    pub fn extra_data_fields(&self) -> impl Iterator<Item = &ExtraField> {
        self.data.extra_fields.0.iter()
    }
}

impl<R: Read + ?Sized> HasZipMetadata for ZipFile<'_, R> {
    fn get_metadata(&self) -> &ZipFileData {
        self.data.as_ref()
    }
}

impl<R: Read + ?Sized> Read for ZipFile<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buf)
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        self.reader.read_to_end(buf)
    }

    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        self.reader.read_to_string(buf)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.reader.read_exact(buf)
    }
}

impl<R: Read> Read for ZipFileSeek<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &mut self.reader {
            ZipFileSeekReader::Raw(r) => r.read(buf),
        }
    }
}

impl<R: Seek> Seek for ZipFileSeek<'_, R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match &mut self.reader {
            ZipFileSeekReader::Raw(r) => r.seek(pos),
        }
    }
}

impl<R> HasZipMetadata for ZipFileSeek<'_, R> {
    fn get_metadata(&self) -> &ZipFileData {
        self.data.as_ref()
    }
}

impl<R: Read + ?Sized> Drop for ZipFile<'_, R> {
    fn drop(&mut self) {
        // self.data is Owned, this reader is constructed by a streaming reader.
        // In this case, we want to exhaust the reader so that the next file is accessible.
        if let Cow::Owned(_) = self.data {
            // Get the inner `Take` reader so all decryption, decompression and CRC calculation is skipped.
            if let Ok(mut inner) = self.take_raw_reader() {
                let _ = copy(&mut inner, &mut sink());
            }
        }
    }
}
