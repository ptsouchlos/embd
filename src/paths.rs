//! This module helps with getting standard paths for use with the `embd` CLI

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::git;

/// Name of the file `embd` stores inside each embedded folder, holding that
/// embed's pinned metadata and file manifest.
pub(crate) const EMBED_FILE: &str = ".embd";

/// Finds the git root of the current directory.
pub(crate) fn find_git_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    git::cli::find_git_root(cwd)
}

/// Get the path to the `.embd` file inside a given embedded folder. Note that
/// this function does not ensure that the file exists.
///
/// # Arguments
///
/// - `folder`: The absolute path to the embedded folder.
///
/// # Returns
/// [`PathBuf`] to the folder's configuration file.
pub(crate) fn embed_file_path(folder: &Path) -> PathBuf {
    folder.join(EMBED_FILE)
}

/// Walk the tree from `root`, skipping `.git` and other well-known
/// non-source directories, and return every folder (relative to `root`) that
/// directly contains a `.embd` embed file. Sorted lexicographically for
/// deterministic output.
///
/// Once a folder's own `.embd` marker is found, its subtree is not recursed
/// into any further — an embed's contents are never themselves walked
/// looking for nested `.embd` files.
///
/// # Arguments
///
/// - `root`: The git root to search from.
///
/// # Returns
/// Sorted folder paths, each relative to `root`.
pub(crate) fn discover_embed_folders(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    discover_inner(root, Path::new(""), &mut out)?;
    out.sort();
    Ok(out)
}

fn discover_inner(root: &Path, relative: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let absolute = root.join(relative);
    let read = std::fs::read_dir(&absolute)
        .with_context(|| format!("failed to read directory {}", absolute.display()))?;

    let mut has_marker = false;
    let mut subdirs = Vec::new();
    for entry in read {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git"
            || name == "target"
            || name == "node_modules"
            || name == ".worktrees"
            || name == "worktrees"
        {
            continue;
        }
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read file type of {}", entry.path().display()))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_file() && name == EMBED_FILE {
            has_marker = true;
            continue;
        }
        if file_type.is_dir() {
            subdirs.push(relative.join(&name));
        }
    }

    if has_marker {
        // The git root itself is never a valid "embed folder" (there's no
        // parent project to embed it into); silently skip it rather than
        // reporting it as discovered.
        if !relative.as_os_str().is_empty() {
            out.push(relative.to_path_buf());
        }
    } else {
        for sub in subdirs {
            discover_inner(root, &sub, out)?;
        }
    }
    Ok(())
}

/// Resolve a user-supplied target folder against the current working directory
/// and verify it lives inside `root`. Returns the path normalized as
/// `(absolute, relative_to_root)`.
///
/// Rejects paths that escape the git root via `..` or absolute paths outside it.
/// Rejects paths that already exist as a non-directory (e.g. a regular file).
/// Rejects paths that are symlinks (we won't follow them).
///
/// # Arguments
///
/// - `folder`: The target folder to resolve.
/// - `root`: The root folder to compare to.
/// - `cwd`: The current working directory.
///
/// # Returns
/// A path pair, normalized as (absolute_path, relative_to_root).
pub(crate) fn resolve_inside_root(
    folder: &Path,
    root: &Path,
    cwd: &Path,
) -> Result<(PathBuf, PathBuf)> {
    let absolute = if folder.is_absolute() {
        folder.to_path_buf()
    } else {
        cwd.join(folder)
    };
    let normalized = lexically_normalize(&absolute);
    let root_normalized = lexically_normalize(root);

    let relative = normalized
        .strip_prefix(&root_normalized)
        .map(Path::to_path_buf)
        .map_err(|_| {
            anyhow::anyhow!(
                "folder '{}' is outside the git root '{}'",
                folder.display(),
                root.display()
            )
        })?;

    if relative.as_os_str().is_empty() {
        bail!("folder cannot be the git root itself");
    }

    if let Ok(meta) = normalized.symlink_metadata() {
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            bail!(
                "folder '{}' is a symlink; refusing to follow",
                folder.display()
            );
        }
        if !meta.is_dir() {
            bail!(
                "folder '{}' exists and is not a directory",
                folder.display()
            );
        }
    }

    Ok((normalized, relative))
}

fn lexically_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in p.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let popped = out.pop();
                if !popped {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn resolves_relative_path_inside_root() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let (abs, rel) = resolve_inside_root(Path::new("vendor/foo"), root, root).unwrap();
        assert_eq!(abs, root.join("vendor").join("foo"));
        assert_eq!(rel, PathBuf::from("vendor").join("foo"));
    }

    #[test]
    fn rejects_path_escaping_root_via_parent_dir() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let result = resolve_inside_root(Path::new("../outside"), root, root);
        assert!(result.is_err());
        assert!(format!("{:#}", result.unwrap_err()).contains("outside the git root"));
    }

    #[test]
    fn rejects_absolute_path_outside_root() {
        let tmp = tempdir().unwrap();
        let other = tempdir().unwrap();
        let result = resolve_inside_root(other.path(), tmp.path(), tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn rejects_root_itself() {
        let tmp = tempdir().unwrap();
        let result = resolve_inside_root(Path::new("."), tmp.path(), tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn rejects_existing_file_at_path() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let file = root.join("regular.txt");
        std::fs::write(&file, "hi").unwrap();
        let result = resolve_inside_root(Path::new("regular.txt"), root, root);
        assert!(result.is_err());
        assert!(format!("{:#}", result.unwrap_err()).contains("not a directory"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_at_path() {
        use std::os::unix::fs::symlink;
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let target = root.join("real");
        std::fs::create_dir(&target).unwrap();
        let link = root.join("link");
        symlink(&target, &link).unwrap();
        let result = resolve_inside_root(Path::new("link"), root, root);
        assert!(result.is_err());
        assert!(format!("{:#}", result.unwrap_err()).contains("symlink"));
    }

    #[test]
    fn accepts_absolute_path_inside_root() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let abs = root.join("nested").join("dir");
        let (_, rel) = resolve_inside_root(&abs, root, root).unwrap();
        assert_eq!(rel, PathBuf::from("nested").join("dir"));
    }

    #[test]
    fn embed_file_path_is_dot_embd_inside_folder() {
        let folder = Path::new("/repo/infra");
        assert_eq!(embed_file_path(folder), PathBuf::from("/repo/infra/.embd"));
    }

    #[test]
    fn discover_finds_all_dot_embd_files_sorted() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("zebra")).unwrap();
        std::fs::write(root.join("zebra/.embd"), "x").unwrap();
        std::fs::create_dir_all(root.join("nested/alpha")).unwrap();
        std::fs::write(root.join("nested/alpha/.embd"), "x").unwrap();

        let found = discover_embed_folders(root).unwrap();
        assert_eq!(
            found,
            vec![
                PathBuf::from("nested").join("alpha"),
                PathBuf::from("zebra"),
            ]
        );
    }

    #[test]
    fn discover_skips_git_directory() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/.embd"), "x").unwrap();
        let found = discover_embed_folders(root).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn discover_returns_empty_for_no_embeds() {
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("plain")).unwrap();
        let found = discover_embed_folders(tmp.path()).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn discover_skips_well_known_ignored_directories() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target/.embd"), "x").unwrap();
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        std::fs::write(root.join("node_modules/.embd"), "x").unwrap();
        std::fs::create_dir_all(root.join(".worktrees")).unwrap();
        std::fs::write(root.join(".worktrees/.embd"), "x").unwrap();
        std::fs::create_dir_all(root.join("worktrees")).unwrap();
        std::fs::write(root.join("worktrees/.embd"), "x").unwrap();

        let found = discover_embed_folders(root).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn discover_does_not_recurse_into_an_already_found_embed() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("outer/inner")).unwrap();
        std::fs::write(root.join("outer/.embd"), "x").unwrap();
        std::fs::write(root.join("outer/inner/.embd"), "x").unwrap();

        let found = discover_embed_folders(root).unwrap();
        assert_eq!(found, vec![PathBuf::from("outer")]);
    }

    #[test]
    fn discover_skips_embd_marker_at_git_root_itself() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join(".embd"), "x").unwrap();

        let found = discover_embed_folders(root).unwrap();
        assert!(found.is_empty());
    }
}
