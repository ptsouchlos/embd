use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::{filesystem, filter::Filter};

/// Walk a directory tree and return every regular file's path relative to
/// `root`, sorted lexicographically. Skips `.git` directories, files rejected by
/// `filter`, and silently ignores symlinks (the status walker reports symlinks
/// separately).
pub(crate) fn walk_files(root: &Path, filter: &Filter) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk_files_inner(root, Path::new(""), filter, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk_files_inner(
    root: &Path,
    relative: &Path,
    filter: &Filter,
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    let absolute = root.join(relative);
    let read = std::fs::read_dir(&absolute)
        .with_context(|| format!("failed to read directory {}", absolute.display()))?;
    for entry in read {
        let entry = entry?;
        let name = entry.file_name();
        if filesystem::is_skipped_entry(&name) {
            continue;
        }
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read file type of {}", entry.path().display()))?;
        let child_relative = relative.join(&name);
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            walk_files_inner(root, &child_relative, filter, out)?;
        } else if file_type.is_file() {
            if !filter.includes(&super::path_to_key(&child_relative)) {
                continue;
            }
            out.push(child_relative);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn walk_files_returns_sorted_relative_paths() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("b/sub")).unwrap();
        std::fs::write(dir.path().join("b/sub/c.txt"), "c").unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("b/b.txt"), "b").unwrap();

        let files = walk_files(dir.path(), &Filter::allow_all()).unwrap();
        assert_eq!(
            files,
            vec![
                PathBuf::from("a.txt"),
                PathBuf::from("b").join("b.txt"),
                PathBuf::from("b").join("sub").join("c.txt"),
            ]
        );
    }

    #[test]
    fn walk_files_skips_git_directory() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "x").unwrap();
        std::fs::write(dir.path().join("keep.txt"), "k").unwrap();
        let files = walk_files(dir.path(), &Filter::allow_all()).unwrap();
        assert_eq!(files, vec![PathBuf::from("keep.txt")]);
    }

    #[cfg(unix)]
    #[test]
    fn walk_files_skips_symlinks() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("real.txt"), "r").unwrap();
        symlink("real.txt", dir.path().join("link.txt")).unwrap();
        let files = walk_files(dir.path(), &Filter::allow_all()).unwrap();
        assert_eq!(files, vec![PathBuf::from("real.txt")]);
    }
}
