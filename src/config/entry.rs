//! Entry type for the configuration file format.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use super::{FileLocks, Metadata};

/// Represents a single entry in the `embd` configuration file. Each embedded
/// folder has its own `.embd` file holding exactly one `EmbdEntry`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbdEntry {
    pub metadata: Metadata,
    #[serde(flatten)]
    pub files: FileLocks,
}

impl EmbdEntry {
    /// Load an entry from its `.embd` file at `path`.
    ///
    /// # Arguments
    /// - `path`: The full path to load the file from.
    ///
    /// # Returns
    /// The loaded entry, or an error if reading or parsing failed.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config from {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("failed to parse config from {}", path.display()))
    }

    /// Save this entry to its `.embd` file at `path`. Uses a tempfile-then-rename
    /// approach so a crash mid-write can't leave a torn file behind.
    ///
    /// # Arguments
    /// - `path`: The full path to save the configuration file to.
    ///
    /// # Returns
    /// An error if the save operation failed in any way.
    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .with_context(|| format!("config path {} has no parent directory", path.display()))?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
        let content = toml::to_string_pretty(self).context("failed to serialize config")?;

        let mut tmp = NamedTempFile::new_in(parent)
            .with_context(|| format!("failed to create temp file in {}", parent.display()))?;
        tmp.write_all(content.as_bytes())
            .context("failed to write config file content")?;
        tmp.persist(path)
            .with_context(|| format!("failed to persist config to {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn entry(remote: &str, commit_hash: &str) -> EmbdEntry {
        EmbdEntry {
            metadata: Metadata {
                remote: remote.to_string(),
                commit_hash: commit_hash.to_string(),
                allow_untracked: false,
                include: Vec::new(),
                exclude: Vec::new(),
            },
            files: FileLocks::default(),
        }
    }

    #[test]
    fn round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".embd");

        let e = entry("https://a.git", "aaa111");
        e.save(&path).unwrap();

        let loaded = EmbdEntry::load(&path).unwrap();
        assert_eq!(loaded.metadata.remote, "https://a.git");
        assert_eq!(loaded.metadata.commit_hash, "aaa111");
        assert_eq!(loaded.files.as_map(), e.files.as_map());
    }

    #[test]
    fn save_creates_parent_directory() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join(".embd");
        entry("https://a.git", "aaa111").save(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn load_parses_toml_format() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".embd");
        std::fs::write(
            &path,
            r#"
[metadata]
remote = "https://example.git"
commit_hash = "abc123"
allow_untracked = false

[files]
"a.txt" = "sha256:abc"
"#,
        )
        .unwrap();
        let loaded = EmbdEntry::load(&path).unwrap();
        assert_eq!(loaded.metadata.remote, "https://example.git");
        assert_eq!(loaded.metadata.commit_hash, "abc123");
        assert_eq!(
            loaded.files.as_map().get("a.txt"),
            Some(&"sha256:abc".to_string())
        );
    }

    #[test]
    fn load_missing_file_errors() {
        let result = EmbdEntry::load(Path::new("/nonexistent/path/.embd"));
        assert!(result.is_err());
        assert!(format!("{:#}", result.unwrap_err()).contains("failed to read config"));
    }

    #[test]
    fn load_rejects_toml_missing_allow_untracked() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".embd");
        std::fs::write(
            &path,
            r#"
[metadata]
remote = "https://example.git"
commit_hash = "abc123"
"#,
        )
        .unwrap();
        let result = EmbdEntry::load(&path);
        assert!(result.is_err(), "missing allow_untracked must be rejected");
        assert!(
            format!("{:#}", result.unwrap_err()).contains("allow_untracked"),
            "error should mention the missing field"
        );
    }

    #[test]
    fn load_malformed_toml_errors() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".embd");
        std::fs::write(&path, "not valid toml {{{{").unwrap();
        let result = EmbdEntry::load(&path);
        assert!(result.is_err());
        assert!(format!("{:#}", result.unwrap_err()).contains("failed to parse config"));
    }
}
