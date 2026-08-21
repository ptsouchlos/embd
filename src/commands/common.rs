use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::EmbdEntry;
use crate::paths;

/// Resolve a list of user-supplied folder paths to `(folder, entry)` pairs,
/// each folder relative to `root`. An empty `folders` slice means "every
/// embed discovered under `root`, in deterministic (sorted) order."
pub(crate) fn select_entries(
    root: &Path,
    folders: &[String],
    cwd: &Path,
) -> Result<Vec<(PathBuf, EmbdEntry)>> {
    if folders.is_empty() {
        let mut out = Vec::new();
        for folder in paths::discover_submodule_folders(root)? {
            let config_path = paths::submodule_file_path(&root.join(&folder));
            let entry = EmbdEntry::load(&config_path)
                .with_context(|| format!("failed to load {}", config_path.display()))?;
            out.push((folder, entry));
        }
        Ok(out)
    } else {
        let mut out = Vec::with_capacity(folders.len());
        for folder_arg in folders {
            let (_, folder_rel) = paths::resolve_inside_root(Path::new(folder_arg), root, cwd)?;
            let config_path = paths::submodule_file_path(&root.join(&folder_rel));
            let entry = EmbdEntry::load(&config_path).with_context(|| {
                format!(
                    "no embed found at '{folder_arg}' ({} not found)",
                    config_path.display()
                )
            })?;
            out.push((folder_rel, entry));
        }
        Ok(out)
    }
}
