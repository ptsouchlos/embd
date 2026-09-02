use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::color;
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
        let discovered = paths::discover_embed_folders(root)?;
        if discovered.is_empty() && root.join(".embd").is_dir() {
            bail!(
                "found a legacy '.embd/' project-level config directory; embd now stores one \
                 .embd file per embedded folder instead of one file per project. See \
                 docs/design.md for the new layout, and convert existing entries by hand \
                 (there is no automated migration)."
            );
        }

        let mut out = Vec::new();
        for folder in discovered {
            let config_path = paths::embed_file_path(&root.join(&folder));
            match EmbdEntry::load(&config_path)
                .context("failed to load embed config found during discovery")
            {
                Ok(entry) => out.push((folder, entry)),
                Err(e) => {
                    anstream::eprintln!(
                        "{} skipping '{}': {e:#}",
                        color::warning_label(),
                        folder.display()
                    );
                }
            }
        }
        Ok(out)
    } else {
        let mut out = Vec::with_capacity(folders.len());
        for folder_arg in folders {
            let (_, folder_rel) = paths::resolve_inside_root(Path::new(folder_arg), root, cwd)?;
            let config_path = paths::embed_file_path(&root.join(&folder_rel));
            let entry = EmbdEntry::load(&config_path)
                .with_context(|| format!("no embed found at '{folder_arg}'"))?;
            out.push((folder_rel, entry));
        }
        Ok(out)
    }
}
