//! This module helps with getting standard paths for use with the `embd` CLI

use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};

use crate::git;

const EMBD_FOLDER: &str = ".embd";
const CONFIG_FILE: &str = "config.toml";
const LOCK_FILE: &str = "embd.lock";

/// Finds the git root of the current directory.
pub(crate) fn find_git_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    git::cli::find_git_root(cwd)
}

/// Get the root folder of where `embd` stores information/data/cache.
///
/// # Arguments
///
/// - `root_path`: The root path.
///
/// # Returns
/// [`PathBuf`] to the project folder.
pub(crate) fn project_folder(root_path: &Path) -> PathBuf {
    root_path.join(EMBD_FOLDER)
}

/// Get the path to the config file in a given root path. Note that this
/// function does not ensure that the file exists.
///
/// # Arguments
///
/// - `root_path`: The root path to search.
///
/// # Returns
/// [`PathBuf`] to the configuration file.
pub(crate) fn config_path(root_path: &Path) -> PathBuf {
    project_folder(root_path).join(CONFIG_FILE)
}

/// Get the path to the consolidated lock file inside the embd project folder.
/// Note that this function does not ensure that the file exists.
pub(crate) fn lock_path(root_path: &Path) -> PathBuf {
    project_folder(root_path).join(LOCK_FILE)
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
                // Don't pop past the root prefix.
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
    fn lock_path_is_inside_project_folder() {
        let root = Path::new("/repo");
        assert_eq!(
            lock_path(root),
            PathBuf::from("/repo/.embd/embd.lock")
        );
    }

    #[test]
    fn accepts_absolute_path_inside_root() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let abs = root.join("nested").join("dir");
        let (_, rel) = resolve_inside_root(&abs, root, root).unwrap();
        assert_eq!(rel, PathBuf::from("nested").join("dir"));
    }
}
