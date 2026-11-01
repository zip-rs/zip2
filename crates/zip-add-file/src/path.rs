//! Filesystem file-adding APIs for ZIP archives.
//!
//! The extension traits add path-based convenience operations to the shared writer types.

pub use zip_write::write::{
    FileOptions, FullFileOptions, PreparedZipFile, SimpleFileOptions, ZipFileBuilder, ZipWriter,
};

use std::io::{Read, Seek, Write};
#[cfg(feature = "rayon")]
use std::num::NonZeroUsize;
use std::path::Path;

use zip_core_lib::result::ZipResult;
use zip_core_lib::write::FileOptionExtension;
use zip_read::read::ZipFile;
use zip_update::ZipUpdate;

/// Filesystem-oriented file addition methods for [`ZipWriter`].
pub trait ZipAddFile: Write {
    /// Reads a filesystem file and stores it in the archive under the normalized path.
    fn add_file_from_path<P: AsRef<Path>, E: FileOptionExtension>(
        &mut self,
        path: P,
        options: FileOptions<'_, '_, E>,
    ) -> ZipResult<()>;

    /// Reads a filesystem file and stores it under an explicit archive path.
    fn add_file_from_path_with_name<P: AsRef<Path>, N: ToString, E: FileOptionExtension>(
        &mut self,
        path: P,
        archive_name: N,
        options: FileOptions<'_, '_, E>,
    ) -> ZipResult<()>;

    /// Starts a file entry whose name is derived from a normalized filesystem path.
    fn start_file_from_path<P: AsRef<Path>, E: FileOptionExtension>(
        &mut self,
        path: P,
        options: FileOptions<'_, '_, E>,
    ) -> ZipResult<()>;

    /// Adds a directory entry named from a normalized filesystem path.
    fn add_directory_from_path<P: AsRef<Path>, E: FileOptionExtension>(
        &mut self,
        path: P,
        options: FileOptions<'_, '_, E>,
    ) -> ZipResult<()>;

    /// Adds a symlink entry after normalizing its name and target paths.
    fn add_symlink_from_path<P: AsRef<Path>, T: AsRef<Path>, E: FileOptionExtension>(
        &mut self,
        path: P,
        target: T,
        options: FileOptions<'_, '_, E>,
    ) -> ZipResult<()>;

    /// Copies an existing entry to a destination derived from a filesystem path.
    fn deep_copy_file_from_path<P: AsRef<Path>, Q: AsRef<Path>>(
        &mut self,
        source: P,
        destination: Q,
    ) -> ZipResult<()>;

    /// Adds an existing entry to the central directory under paths normalized from filesystem paths.
    fn shallow_copy_file_from_path<P: AsRef<Path>, Q: AsRef<Path>>(
        &mut self,
        source: P,
        destination: Q,
    ) -> ZipResult<()>;

    /// Raw-copies an entry under a normalized filesystem path.
    fn raw_copy_file_to_path<R: Read, P: AsRef<Path>>(
        &mut self,
        file: ZipFile<'_, R>,
        path: P,
    ) -> ZipResult<()>;

    #[cfg(feature = "rayon")]
    fn par_append_from_paths<'a, I, N, P, T>(&mut self, entries: I) -> ZipResult<()>
    where
        Self: Sized + Send,
        I: rayon::iter::IntoParallelIterator<Item = (N, P, FileOptions<'a, 'a, T>)>,
        N: AsRef<str> + Send,
        P: AsRef<Path> + Send,
        T: FileOptionExtension + Send + Sync + 'a;

    #[cfg(feature = "rayon")]
    fn par_append_from_paths_with_capacity<'a, I, N, P, T>(
        &mut self,
        entries: I,
        channel_capacity: NonZeroUsize,
    ) -> ZipResult<()>
    where
        Self: Sized + Send,
        I: rayon::iter::IntoParallelIterator<Item = (N, P, FileOptions<'a, 'a, T>)>,
        N: AsRef<str> + Send,
        P: AsRef<Path> + Send,
        T: FileOptionExtension + Send + Sync + 'a;

    #[cfg(feature = "rayon")]
    fn par_append_from_paths_atomic<'a, I, N, P, T>(&mut self, entries: I) -> ZipResult<()>
    where
        Self: Sized,
        I: rayon::iter::IntoParallelIterator<Item = (N, P, FileOptions<'a, 'a, T>)>,
        N: AsRef<str> + Send,
        P: AsRef<Path> + Send,
        T: FileOptionExtension + Send + Sync + 'a;
}

impl<W: Read + Write + Seek> ZipAddFile for ZipWriter<W> {
    fn add_file_from_path<P: AsRef<Path>, E: FileOptionExtension>(
        &mut self,
        path: P,
        options: FileOptions<'_, '_, E>,
    ) -> ZipResult<()> {
        let archive_name = zip_core_lib::unstable::path_to_string(path.as_ref())?;
        self.add_file_from_path_with_name(path, archive_name, options)
    }

    fn add_file_from_path_with_name<P: AsRef<Path>, N: ToString, E: FileOptionExtension>(
        &mut self,
        path: P,
        archive_name: N,
        options: FileOptions<'_, '_, E>,
    ) -> ZipResult<()> {
        let mut file = std::fs::File::open(path)?;
        self.start_file(archive_name.to_string(), options)?;
        std::io::copy(&mut file, self)?;
        Ok(())
    }

    fn start_file_from_path<P: AsRef<Path>, E: FileOptionExtension>(
        &mut self,
        path: P,
        options: FileOptions<'_, '_, E>,
    ) -> ZipResult<()> {
        self.start_file(zip_core_lib::unstable::path_to_string(path)?, options)
    }

    fn add_directory_from_path<P: AsRef<Path>, E: FileOptionExtension>(
        &mut self,
        path: P,
        options: FileOptions<'_, '_, E>,
    ) -> ZipResult<()> {
        self.add_directory(zip_core_lib::unstable::path_to_string(path)?, options)
    }

    fn add_symlink_from_path<P: AsRef<Path>, T: AsRef<Path>, E: FileOptionExtension>(
        &mut self,
        path: P,
        target: T,
        options: FileOptions<'_, '_, E>,
    ) -> ZipResult<()> {
        self.add_symlink(
            zip_core_lib::unstable::path_to_string(path)?,
            zip_core_lib::unstable::path_to_string(target)?,
            options,
        )
    }

    fn deep_copy_file_from_path<P: AsRef<Path>, Q: AsRef<Path>>(
        &mut self,
        source: P,
        destination: Q,
    ) -> ZipResult<()> {
        self.deep_copy_file(
            &zip_core_lib::unstable::path_to_string(source)?,
            &zip_core_lib::unstable::path_to_string(destination)?,
        )
    }

    fn shallow_copy_file_from_path<P: AsRef<Path>, Q: AsRef<Path>>(
        &mut self,
        source: P,
        destination: Q,
    ) -> ZipResult<()> {
        self.shallow_copy_file(
            &zip_core_lib::unstable::path_to_string(source)?,
            &zip_core_lib::unstable::path_to_string(destination)?,
        )
    }

    fn raw_copy_file_to_path<R: Read, P: AsRef<Path>>(
        &mut self,
        file: ZipFile<'_, R>,
        path: P,
    ) -> ZipResult<()> {
        self.raw_copy_file_rename(file, zip_core_lib::unstable::path_to_string(path)?)
    }

    #[cfg(feature = "rayon")]
    fn par_append_from_paths<'a, I, N, P, T>(&mut self, entries: I) -> ZipResult<()>
    where
        Self: Sized + Send,
        I: rayon::iter::IntoParallelIterator<Item = (N, P, FileOptions<'a, 'a, T>)>,
        N: AsRef<str> + Send,
        P: AsRef<Path> + Send,
        T: FileOptionExtension + Send + Sync + 'a,
    {
        self.par_append_from_paths_with_capacity(
            entries,
            NonZeroUsize::new(rayon::current_num_threads().saturating_mul(2))
                .unwrap_or(NonZeroUsize::MIN),
        )
    }

    #[cfg(feature = "rayon")]
    fn par_append_from_paths_with_capacity<'a, I, N, P, T>(
        &mut self,
        entries: I,
        channel_capacity: NonZeroUsize,
    ) -> ZipResult<()>
    where
        Self: Sized + Send,
        I: rayon::iter::IntoParallelIterator<Item = (N, P, FileOptions<'a, 'a, T>)>,
        N: AsRef<str> + Send,
        P: AsRef<Path> + Send,
        T: FileOptionExtension + Send + Sync + 'a,
    {
        use rayon::prelude::*;
        use std::collections::HashSet;
        use std::sync::Mutex;
        use std::sync::mpsc::sync_channel;

        let par_iter = entries.into_par_iter();
        let additional = par_iter.opt_len().unwrap_or(0);
        let mut initial_seen = HashSet::with_capacity(additional);
        for key in self.existing_file_names() {
            initial_seen.insert(key);
        }
        let seen = Mutex::new(initial_seen);
        let (tx, rx) = sync_channel(channel_capacity.get());
        let mut write_err = None;

        std::thread::scope(|scope| {
            let consumer = scope.spawn(|| -> ZipResult<()> {
                for item in rx {
                    self.add_prepared_file(item?)?;
                }
                Ok(())
            });

            let _ = par_iter.try_for_each(|(name, path, options)| -> Result<(), ()> {
                let name_str = name.as_ref();
                let name_bytes: Box<[u8]> = options.name.unwrap_or(name_str.as_bytes()).into();
                {
                    let mut guard = match seen.lock() {
                        Ok(guard) => guard,
                        Err(_) => return Err(()),
                    };
                    if !guard.insert(name_bytes) {
                        let _ = tx.send(Err(zip_core_lib::result::invalid!(
                            "Duplicate filename: {:?}",
                            name_str
                        )));
                        return Err(());
                    }
                }
                let prepared = match PreparedZipFile::from_path(name, path, options) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        return Err(());
                    }
                };
                if tx.send(Ok(prepared)).is_err() {
                    return Err(());
                }
                Ok(())
            });

            drop(tx);
            if let Ok(result) = consumer.join() {
                write_err = result.err();
            }
        });

        if let Some(error) = write_err {
            return Err(error);
        }
        Ok(())
    }

    #[cfg(feature = "rayon")]
    fn par_append_from_paths_atomic<'a, I, N, P, T>(&mut self, entries: I) -> ZipResult<()>
    where
        Self: Sized,
        I: rayon::iter::IntoParallelIterator<Item = (N, P, FileOptions<'a, 'a, T>)>,
        N: AsRef<str> + Send,
        P: AsRef<Path> + Send,
        T: FileOptionExtension + Send + Sync + 'a,
    {
        use rayon::prelude::*;

        let prepared_files: ZipResult<Vec<PreparedZipFile>> = entries
            .into_par_iter()
            .map(|(name, path, options)| PreparedZipFile::from_path(name, path, options))
            .collect();
        let prepared_files = prepared_files?;

        let mut seen: std::collections::HashSet<Box<[u8]>> =
            self.existing_file_names().into_iter().collect();
        for file in &prepared_files {
            let name: Box<[u8]> = file.file_name_raw().into();
            if !seen.insert(name) {
                return Err(zip_core_lib::result::invalid!(
                    "Duplicate filename: {:?}",
                    file.file_name_raw().escape_ascii()
                ));
            }
        }
        for file in prepared_files {
            self.add_prepared_file(file)?;
        }
        Ok(())
    }
}

/// Path-based constructor for [`ZipFileBuilder`].
pub trait ZipFileBuilderPathExt {
    /// Constructs a builder with an archive name normalized from a filesystem path.
    fn new_from_path<T: FileOptionExtension, P: AsRef<Path>>(
        path: P,
        options: FileOptions<'_, '_, T>,
    ) -> ZipResult<ZipFileBuilder>;
}

impl ZipFileBuilderPathExt for ZipFileBuilder {
    fn new_from_path<T: FileOptionExtension, P: AsRef<Path>>(
        path: P,
        options: FileOptions<'_, '_, T>,
    ) -> ZipResult<ZipFileBuilder> {
        ZipFileBuilder::new(zip_core_lib::unstable::path_to_string(path)?, options)
    }
}

/// Filesystem constructor for [`PreparedZipFile`].
pub trait PreparedZipFilePathExt {
    /// Reads and compresses a filesystem file using an explicit archive entry name.
    fn from_path<N: AsRef<str>, P: AsRef<Path>, T: FileOptionExtension>(
        archive_name: N,
        disk_path: P,
        options: FileOptions<'_, '_, T>,
    ) -> ZipResult<PreparedZipFile>;
}

impl PreparedZipFilePathExt for PreparedZipFile {
    fn from_path<N: AsRef<str>, P: AsRef<Path>, T: FileOptionExtension>(
        archive_name: N,
        disk_path: P,
        options: FileOptions<'_, '_, T>,
    ) -> ZipResult<PreparedZipFile> {
        let mut file = std::fs::File::open(disk_path)?;
        let mut builder = ZipFileBuilder::new(archive_name, options)?;
        std::io::copy(&mut file, &mut builder)?;
        builder.finish()
    }
}
