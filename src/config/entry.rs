//! Entry type for the configuration file format

use serde::{Deserialize, Serialize};

use super::{FileLocks, Metadata};

/// Represents a single entry in the `embd` configuration file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbdEntry {
    pub metadata: Metadata,
    #[serde(flatten)]
    pub files: FileLocks,
}
