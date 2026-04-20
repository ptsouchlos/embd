use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Represents a single entry in the `embd` configuration file.
#[derive(Debug, Serialize, Deserialize)]
pub struct EmbdEntry {
    pub remote: String,
    pub commit_hash: String,
    pub folder: PathBuf,
}

/// Represents a configuration for `embd` for a single project/directory.
/// This does not represent a global configuration for the CLI on a given system.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config(HashMap<String, EmbdEntry>);

impl Config {
    /// Load the configuration from a given path.
    ///
    /// # Arguments
    /// - `path`: The full path to load the file from.
    ///
    /// # Returns
    /// The loaded configuration object, or an error if it failed.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config from {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("failed to parse config from {}", path.display()))
    }

    /// Save the current config to the given path.
    ///
    /// # Arguments
    /// - `path`: The full path to save the configuration file to.
    ///
    /// # Returns
    /// An error if the save operation failed in any way.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }
        let content = toml::to_string_pretty(self).context("failed to serialize config")?;
        std::fs::write(path, content)
            .with_context(|| format!("failed to write config to {}", path.display()))
    }

    /// Insert a new entry into the configuration. This does not save the entry to the file.
    ///
    /// # Arguments
    /// - `name`: The name/identifier of the entry. This must be unique.
    /// - `entry`: The new entry to add to the config.
    ///
    /// # Returns
    /// Error if the name was not unique.
    pub fn insert(&mut self, name: String, entry: EmbdEntry) -> Result<()> {
        if self.contains(name.as_str()) {
            bail!("{name} is not a unique key.")
        }

        let _old_value = self.0.insert(name, entry);
        Ok(())
    }

    /// Get an entry for the given name.
    ///
    /// # Arguments
    ///
    /// - `name`: The name/identifier of the entry.
    ///
    /// # Returns
    /// An [`EmbdEntry`] if one exists at the given identifier. None otherwise.
    pub fn get(&self, name: &str) -> Option<&EmbdEntry> {
        self.0.get(name)
    }

    /// Check if the configuration contains an entry for the given name.
    ///
    /// # Arguments
    /// - `name`: The name/identifier to check.
    ///
    /// # Returns
    /// True if the configuration contains the the identifier, false otherwise.
    pub fn contains(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn entry(remote: &str, commit_hash: &str, folder: &str) -> EmbdEntry {
        EmbdEntry {
            remote: remote.to_string(),
            commit_hash: commit_hash.to_string(),
            folder: PathBuf::from(folder),
        }
    }

    #[test]
    fn default_is_empty() {
        assert!(!Config::default().contains("anything"));
    }

    #[test]
    fn insert_and_contains() {
        let mut config = Config::default();
        assert!(!config.contains("mylib"));
        let insert_result = config.insert(
            "mylib".to_string(),
            entry("https://example.git", "abc123", "third_party/mylib"),
        );
        assert!(insert_result.is_ok());
        assert!(config.contains("mylib"));
        assert!(!config.contains("other"));
    }

    #[test]
    fn insert_existing_key_fails() {
        let mut config = Config::default();
        config
            .insert(
                "mylib".to_string(),
                entry("https://example.git", "abc123", "third_party/mylib"),
            )
            .unwrap();
        let result = config.insert(
            "mylib".to_string(),
            entry("https://other.git", "def456", "third_party/other"),
        );
        assert!(result.is_err());
        assert!(config.contains("mylib"));
    }

    #[test]
    fn get_returns_entry() {
        let mut config = Config::default();
        config
            .insert(
                "mylib".to_string(),
                entry("https://example.git", "abc123", "third_party/mylib"),
            )
            .unwrap();
        let e = config.get("mylib").unwrap();
        assert_eq!(e.remote, "https://example.git");
        assert_eq!(e.commit_hash, "abc123");
        assert_eq!(e.folder, PathBuf::from("third_party/mylib"));
        assert!(config.get("missing").is_none());
    }

    #[test]
    fn round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let mut config = Config::default();
        config
            .insert(
                "repo1".to_string(),
                entry("https://a.git", "aaa111", "third_party/a"),
            )
            .unwrap();
        config
            .insert(
                "repo2".to_string(),
                entry("https://b.git", "bbb222", "vendor/b"),
            )
            .unwrap();
        config.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        let e1 = loaded.get("repo1").unwrap();
        assert_eq!(e1.remote, "https://a.git");
        assert_eq!(e1.commit_hash, "aaa111");
        assert_eq!(e1.folder, PathBuf::from("third_party/a"));
        assert!(loaded.get("repo2").is_some());
    }

    #[test]
    fn save_creates_parent_directory() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(".embd").join("config.toml");
        Config::default().save(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn load_parses_toml_format() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[repo1]
remote = "https://example.git"
commit_hash = "abc123"
folder = "third_party/repo1"
"#,
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        let e = config.get("repo1").unwrap();
        assert_eq!(e.remote, "https://example.git");
        assert_eq!(e.commit_hash, "abc123");
    }

    #[test]
    fn load_missing_file_errors() {
        let result = Config::load(Path::new("/nonexistent/path/config.toml"));
        assert!(result.is_err());
        assert!(format!("{:#}", result.unwrap_err()).contains("failed to read config"));
    }

    #[test]
    fn load_malformed_toml_errors() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not valid toml {{{{").unwrap();
        let result = Config::load(&path);
        assert!(result.is_err());
        assert!(format!("{:#}", result.unwrap_err()).contains("failed to parse config"));
    }
}
