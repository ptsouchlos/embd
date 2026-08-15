use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Represents the metadata stored for a single entry in `embd`'s configuration file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub remote: String,
    pub commit_hash: String,
    pub folder: PathBuf,
    pub allow_untracked: bool,
    /// Glob patterns; when non-empty, only matching files are pulled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Glob patterns; matching files are never pulled (excludes win over includes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}
