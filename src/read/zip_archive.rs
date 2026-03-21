//! Code related to `ZipArchive`

use crate::read::config::Config;
use indexmap::IndexMap;
use std::sync::Arc;
use crate::types::ZipFileData;

/// Immutable metadata about a `ZipArchive`.
#[derive(Debug)]
pub struct ZipArchiveMetadata {
    pub(crate) files: IndexMap<Box<str>, ZipFileData>,
    pub(crate) offset: u64,
    pub(crate) dir_start: u64,
    // This isn't yet used anywhere, but it is here for use cases in the future.
    #[allow(dead_code)]
    pub(crate) config: Config,
    pub(crate) comment: Box<[u8]>,
    pub(crate) zip64_comment: Option<Box<[u8]>>,
}


#[derive(Debug)]
pub(crate) struct SharedBuilder {
    pub(crate) files: Vec<super::ZipFileData>,
    pub(super) offset: u64,
    pub(super) dir_start: u64,
    // This isn't yet used anywhere, but it is here for use cases in the future.
    #[allow(dead_code)]
    pub(super) config: super::Config,
}

impl SharedBuilder {
    pub fn build(
        self,
        comment: Box<[u8]>,
        zip64_comment: Option<Box<[u8]>>,
    ) -> ZipArchiveMetadata {
        let mut index_map = IndexMap::with_capacity(self.files.len());
        self.files.into_iter().for_each(|file| {
            index_map.insert(file.file_name.clone(), file);
        });
        ZipArchiveMetadata {
            files: index_map,
            offset: self.offset,
            dir_start: self.dir_start,
            config: self.config,
            comment,
            zip64_comment,
        }
    }
}

/// ZIP archive reader
///
/// At the moment, this type is cheap to clone if this is the case for the
/// reader it uses. However, this is not guaranteed by this crate and it may
/// change in the future.
///
/// ```no_run
/// use std::io::{Read, Seek};
/// fn list_zip_contents(reader: impl Read + Seek) -> zip::result::ZipResult<()> {
///     use zip::HasZipMetadata;
///     let mut zip = zip::ZipArchive::new(reader)?;
///
///     for i in 0..zip.len() {
///         let mut file = zip.by_index(i)?;
///         println!("Filename: {}", file.name());
///         std::io::copy(&mut file, &mut std::io::stdout())?;
///     }
///
///     Ok(())
/// }
/// ```
#[derive(Clone, Debug)]
pub struct ZipArchive<R> {
    pub(super) reader: R,
    pub(super) shared: Arc<ZipArchiveMetadata>,
}

