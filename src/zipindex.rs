//! An order-preserving lookup map for ZIP file readers

use crate::types::ZipFileData;
use indexmap::IndexMap;
use std::collections::HashMap;

/// Immutable filename lookup map which implements an IndexMap-like interface
/// and can be easily converted from/into IndexMap<Box<str>, ZipFileData>, but 
/// uses significantly less memory and is faster to construct and do lookups.
///
/// # Safety
/// Hashmap keys hold raw pointers to `file_name` boxed strings in `entries`.
#[derive(Debug, Default)]
pub struct ZipIndex {
    entries: Vec<ZipFileData>,
    hashmap: HashMap<&'static str, u32>,
}

impl ZipIndex {
    /// Build a ZipIndex from entries, deduplicating by file name.
    /// Later entries with the same name replace earlier ones in-place.
    pub fn from_vec(entries: Vec<ZipFileData>) -> Self {
        if entries.len() > u32::MAX as usize {
            panic!("ZipIndex cannot contain more than 2^32 entries");
        }
        let mut hashmap: HashMap<&'static str, u32> = HashMap::with_capacity(entries.len());
        let mut result_entries: Vec<ZipFileData> = Vec::with_capacity(entries.len());

        for data in entries {
            // SAFETY: Pointer to boxed string is valid for lifetime of result_entries.
            if let Some(&existing_idx) = hashmap.get(data.file_name.as_ref()) {
                // Replace existing entry in-place, retain existing boxed string.
                let existing_entry =
                    std::mem::replace(&mut result_entries[existing_idx as usize], data);
                result_entries[existing_idx as usize].file_name = existing_entry.file_name;
            } else {
                // Push new entry, use key derived from pointer to boxed string.
                let idx = result_entries.len() as u32;
                result_entries.push(data);
                let key: &'static str =
                    unsafe { &*(result_entries[idx as usize].file_name.as_ref() as *const str) };
                hashmap.insert(key, idx);
            }
        }

        Self {
            entries: result_entries,
            hashmap,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[allow(clippy::borrowed_box)]
    pub fn get_index(&self, index: usize) -> Option<(&Box<str>, &ZipFileData)> {
        self.entries.get(index).map(|data| (&data.file_name, data))
    }

    pub fn get_index_of(&self, file_name: &str) -> Option<usize> {
        self.hashmap.get(file_name).map(|&idx| idx as usize)
    }

    pub fn contains_key(&self, file_name: &str) -> bool {
        self.hashmap.contains_key(file_name)
    }

    pub fn keys(&self) -> impl Iterator<Item = &Box<str>> {
        self.entries.iter().map(|data| &data.file_name)
    }

    pub fn values(&self) -> impl Iterator<Item = &ZipFileData> {
        self.entries.iter()
    }

    pub fn into_values(self) -> impl IntoIterator<Item = ZipFileData> {
        self.entries.into_iter()
    }
}

impl Clone for ZipIndex {
    fn clone(&self) -> Self {
        let entries = self.entries.clone();
        let mut hashmap: HashMap<&'static str, u32> = HashMap::with_capacity(entries.len());
        for (idx, data) in entries.iter().enumerate() {
            let key: &'static str = unsafe { &*(data.file_name.as_ref() as *const str) };
            hashmap.insert(key, idx as u32);
        }
        Self { entries, hashmap }
    }
}

impl From<IndexMap<Box<str>, ZipFileData>> for ZipIndex {
    fn from(indexmap: IndexMap<Box<str>, ZipFileData>) -> Self {
        Self::from_vec(
            indexmap
                .into_iter()
                .map(|(file_name, data)| {
                    assert_eq!(file_name.as_ref(), data.file_name.as_ref());
                    data
                })
                .collect(),
        )
    }
}

impl From<ZipIndex> for IndexMap<Box<str>, ZipFileData> {
    fn from(zipindex: ZipIndex) -> Self {
        zipindex
            .entries
            .into_iter()
            .map(|data| (data.file_name.clone(), data))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple() {
        let entries = vec![
            ZipFileData {
                file_name: "a.txt".into(),
                ..Default::default()
            },
            ZipFileData {
                file_name: "b.txt".into(),
                ..Default::default()
            },
        ];
        let index = ZipIndex::from_vec(entries);
        assert_eq!(index.len(), 2);
        assert!(index.get_index_of("a.txt").is_some());
        assert!(index.get_index_of("b.txt").is_some());
    }

    #[test]
    fn test_distribution() {
        let entries: Vec<ZipFileData> = (0..100)
            .map(|i| ZipFileData {
                file_name: format!("file_{:03}.txt", i).into(),
                ..Default::default()
            })
            .collect();
        let index = ZipIndex::from_vec(entries.clone());

        for entry in &entries {
            let result = index.get_index_of(entry.file_name.as_ref());
            assert!(result.is_some(), "Failed to find {}", entry.file_name);
        }
    }

    #[test]
    fn test_duplicate_filenames() {
        let mut files = Vec::new();
        for dir in 0..10 {
            for file in 0..12 {
                files.push(ZipFileData {
                    file_name: format!("dir{}/file{}.txt", dir, file).into(),
                    file_comment: "original".into(),
                    ..Default::default()
                });
                if file == 1 || file == 10 {
                    files.push(ZipFileData {
                        file_name: format!("dir{}/file{}.txt", dir, file).into(),
                        file_comment: "duplicate".into(),
                        ..Default::default()
                    });
                }
            }
        }

        let index = ZipIndex::from_vec(files.clone());
        assert_eq!(index.len(), 120);

        for dir in 0..10 {
            for file in [1, 10] {
                let i = index
                    .get_index_of(&format!("dir{}/file{}.txt", dir, file))
                    .unwrap();
                assert_eq!(
                    "duplicate",
                    index.get_index(i).unwrap().1.file_comment.as_ref()
                );
            }
        }
    }

    #[test]
    fn test_dedup_preserves_order() {
        let file1 = ZipFileData {
            file_name: "test.txt".into(),
            file_comment: "first".into(),
            ..Default::default()
        };
        let file2 = ZipFileData {
            file_name: "test.txt".into(),
            file_comment: "second".into(),
            ..Default::default()
        };
        let file3 = ZipFileData {
            file_name: "test.txt".into(),
            file_comment: "third".into(),
            ..Default::default()
        };
        let other = ZipFileData {
            file_name: "other.txt".into(),
            ..Default::default()
        };

        let index = ZipIndex::from_vec(vec![
            file1.clone(),
            other.clone(),
            file2.clone(),
            file3.clone(),
        ]);
        assert_eq!(index.len(), 2);
        assert_eq!(index.get_index_of("test.txt"), Some(0));
        assert_eq!(index.get_index_of("other.txt"), Some(1));
        assert_eq!(index.entries[0].file_comment.as_ref(), "third");
    }

    #[test]
    fn bench_zipindex_vs_indexmap_5m() {
        let entries: Vec<ZipFileData> = (0..5_000_000)
            .map(|i| ZipFileData {
                file_name: format!("file_{:07}.txt", i).into(),
                ..Default::default()
            })
            .collect();

        let start = std::time::Instant::now();
        let zipindex = ZipIndex::from_vec(entries.clone());
        let zipindex_build_time = start.elapsed();

        let start = std::time::Instant::now();
        let indexmap: IndexMap<Box<str>, ZipFileData> = entries
            .clone()
            .into_iter()
            .map(|data| (data.file_name.clone(), data))
            .collect();
        let indexmap_build_time = start.elapsed();

        eprintln!("\n=== BUILD TIME ===");
        eprintln!("ZipIndex:  {:?}", zipindex_build_time);
        eprintln!("IndexMap:  {:?}", indexmap_build_time);
        eprintln!(
            "Ratio:     {:.2}x",
            indexmap_build_time.as_secs_f64() / zipindex_build_time.as_secs_f64()
        );

        let start = std::time::Instant::now();
        for entry in &entries {
            let _ = zipindex.get_index_of(entry.file_name.as_ref());
        }
        let zipindex_lookup_time = start.elapsed();

        let start = std::time::Instant::now();
        for entry in &entries {
            let _ = indexmap.get(entry.file_name.as_ref());
        }
        let indexmap_lookup_time = start.elapsed();

        eprintln!("\n=== SEQUENTIAL LOOKUP TIME ===");
        eprintln!("ZipIndex:  {:?}", zipindex_lookup_time);
        eprintln!("IndexMap:  {:?}", indexmap_lookup_time);
        eprintln!(
            "Ratio:     {:.2}x",
            indexmap_lookup_time.as_secs_f64() / zipindex_lookup_time.as_secs_f64()
        );
    }
}
