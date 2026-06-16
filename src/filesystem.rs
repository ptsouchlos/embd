//! This module contains helpers for the filesystem.

use std::ffi::OsStr;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::cache::path_to_key;
use crate::filter::Filter;

/// Return true if a directory entry should be skipped when walking embedded
/// folders (both during copy and during status hashing). Centralizing the rule
/// here keeps the two walkers in sync.
pub(crate) fn is_skipped_entry(name: &OsStr) -> bool {
    name == ".git"
}

/// Recursively copy a directory from `src` to `dst`, ignoring any `.git`
/// directories and any file rejected by `filter`.
///
/// This doesn't follow symlinks. If a symlink is encountered in `src` the copy
/// fails with an error. This avoids escaping the source repo via a symlink.
///
/// # Arguments
///
/// - `src`: The source path.
/// - `dst`: The destination path.
/// - `filter`: Decides which files are pulled. Use [`Filter::allow_all`] to copy
///   everything.
///
/// # Returns
/// Ok if the copy succeeded, error otherwise.
pub(crate) fn copy_dir(src: &Path, dst: &Path, filter: &Filter) -> Result<()> {
    std::fs::create_dir_all(dst)
        .with_context(|| format!("failed to create directory {}", dst.display()))?;
    copy_dir_inner(src, dst, Path::new(""), filter)
}

fn copy_dir_inner(
    src_root: &Path,
    dst_root: &Path,
    relative: &Path,
    filter: &Filter,
) -> Result<()> {
    let src = src_root.join(relative);
    for entry in std::fs::read_dir(&src)
        .with_context(|| format!("failed to read directory {}", src.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        if is_skipped_entry(&name) {
            continue;
        }
        let src_path = entry.path();
        let child_relative = relative.join(&name);
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read file type of {}", src_path.display()))?;
        if file_type.is_symlink() {
            bail!(
                "encountered symlink at {}; refusing to copy",
                src_path.display()
            );
        }
        if file_type.is_dir() {
            copy_dir_inner(src_root, dst_root, &child_relative, filter)?;
        } else if file_type.is_file() {
            if !filter.includes(&path_to_key(&child_relative)) {
                continue;
            }
            let dst_path = dst_root.join(&child_relative);
            // Create the parent on demand so fully-filtered subtrees don't leave
            // empty directories behind.
            if let Some(parent) = dst_path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create directory {}", parent.display()))?;
            }
            std::fs::copy(&src_path, &dst_path)
                .with_context(|| format!("failed to copy {}", src_path.display()))?;
        } else {
            bail!(
                "encountered unsupported file type at {}",
                src_path.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn copies_files_and_subdirs() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), "a").unwrap();
        std::fs::write(src.join("sub").join("b.txt"), "b").unwrap();

        copy_dir(&src, &dst, &Filter::allow_all()).unwrap();

        assert_eq!(std::fs::read_to_string(dst.join("a.txt")).unwrap(), "a");
        assert_eq!(
            std::fs::read_to_string(dst.join("sub").join("b.txt")).unwrap(),
            "b"
        );
    }

    #[test]
    fn exclude_pattern_drops_files() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(src.join("docs")).unwrap();
        std::fs::write(src.join("keep.rs"), "k").unwrap();
        std::fs::write(src.join("notes.md"), "n").unwrap();
        std::fs::write(src.join("docs").join("guide.md"), "g").unwrap();

        let filter =
            Filter::from_patterns(&[], &["**/*.md".to_string(), "docs/**".to_string()]).unwrap();
        copy_dir(&src, &dst, &filter).unwrap();

        assert!(dst.join("keep.rs").exists());
        assert!(!dst.join("notes.md").exists());
        // The whole excluded subtree is gone, including its directory.
        assert!(!dst.join("docs").exists());
    }

    #[test]
    fn include_pattern_keeps_only_matches() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(src.join("src")).unwrap();
        std::fs::write(src.join("src").join("lib.rs"), "l").unwrap();
        std::fs::write(src.join("Cargo.toml"), "c").unwrap();

        let filter = Filter::from_patterns(&["src/**".to_string()], &[]).unwrap();
        copy_dir(&src, &dst, &filter).unwrap();

        assert!(dst.join("src").join("lib.rs").exists());
        assert!(!dst.join("Cargo.toml").exists());
    }

    #[test]
    fn skips_git_directory() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(src.join(".git")).unwrap();
        std::fs::write(src.join(".git").join("HEAD"), "x").unwrap();
        std::fs::write(src.join("keep.txt"), "k").unwrap();

        copy_dir(&src, &dst, &Filter::allow_all()).unwrap();
        assert!(dst.join("keep.txt").exists());
        assert!(!dst.join(".git").exists());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_copy_symlinks() {
        use std::os::unix::fs::symlink;
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("real.txt"), "r").unwrap();
        symlink("real.txt", src.join("link.txt")).unwrap();

        let err = copy_dir(&src, &dst, &Filter::allow_all()).unwrap_err();
        assert!(format!("{:#}", err).contains("symlink"));
    }
}
