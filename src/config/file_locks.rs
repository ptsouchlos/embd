//! Contains the file "locks" structure and logic for building a tree of
//! file paths along with their SHA256 hash.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::filter::Filter;

use super::{hash, path_to_key, walk};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FileLocks {
    // "Lock file" content. Pairs of file path from root and a sha256 hash.
    files: BTreeMap<String, String>,
}

impl FileLocks {
    pub(crate) fn build_from_path(root: &Path) -> Result<Self> {
        Self::build_from_path_filtered(root, &Filter::allow_all())
    }

    /// Like [`FileLocks::build_from_path`], but only hashes files accepted by
    /// `filter`. Used by `update` so the rebuilt entry reflects the same
    /// include/exclude rules `add` applied.
    pub(crate) fn build_from_path_filtered(root: &Path, filter: &Filter) -> Result<Self> {
        let mut files = BTreeMap::new();
        for relative in walk::walk_files(root, filter)? {
            let absolute = root.join(&relative);
            let hash = hash::hash_file(&absolute)?;
            files.insert(path_to_key(&relative), hash);
        }
        Ok(Self { files })
    }

    /// Borrow the underlying path -> sha256 hash map.
    pub(crate) fn as_map(&self) -> &BTreeMap<String, String> {
        &self.files
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn build_file_locks_from_path() {
        let base_path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "docs"].iter().collect();
        let files = FileLocks::build_from_path(base_path.as_path()).unwrap();
        assert!(!files.as_map().is_empty());
    }

    #[test]
    fn build_file_locks_from_path_recursive() {
        let base_path: PathBuf = [env!("CARGO_MANIFEST_DIR")].iter().collect();
        let mut src_path = base_path.clone();
        src_path.push("src");
        let files = FileLocks::build_from_path(src_path.as_path()).unwrap();
        assert!(!files.as_map().is_empty());

        let readme_pattern = String::from("README.md");
        let files_filtered = FileLocks::build_from_path_filtered(
            base_path.as_path(),
            &Filter::from_patterns(&[readme_pattern], &[]).unwrap(),
        )
        .unwrap();

        assert!(!files_filtered.as_map().is_empty())
    }
}
