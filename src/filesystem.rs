//! This module contains helpers for the filesystem.

use std::ffi::OsStr;
use std::path::Path;

use anyhow::{Context, Result, bail};

/// Return true if a directory entry should be skipped when walking embedded
/// folders (both during copy and during status hashing). Centralizing the rule
/// here keeps the two walkers in sync.
pub(crate) fn is_skipped_entry(name: &OsStr) -> bool {
    name == ".git"
}

/// Recursively copy a directory from `src` to `dst`, ignoring any `.git` directories.
///
/// This doesn't follow symlinks. If a symlink is encountered in `src` the copy
/// fails with an error. This avoids escaping the source repo via a symlink.
///
/// # Arguments
///
/// - `src`: The source path.
/// - `dst`: The destination path.
///
/// # Returns
/// Ok if the copy succeeded, error otherwise.
pub(crate) fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)
        .with_context(|| format!("failed to create directory {}", dst.display()))?;
    for entry in std::fs::read_dir(src)
        .with_context(|| format!("failed to read directory {}", src.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        if is_skipped_entry(&name) {
            println!("Skipping {}", name.to_str().unwrap_or("failed"));
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(&name);
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
            copy_dir(&src_path, &dst_path)?;
        } else if file_type.is_file() {
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

        copy_dir(&src, &dst).unwrap();

        assert_eq!(std::fs::read_to_string(dst.join("a.txt")).unwrap(), "a");
        assert_eq!(
            std::fs::read_to_string(dst.join("sub").join("b.txt")).unwrap(),
            "b"
        );
    }

    #[test]
    fn skips_git_directory() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(src.join(".git")).unwrap();
        std::fs::write(src.join(".git").join("HEAD"), "x").unwrap();
        std::fs::write(src.join("keep.txt"), "k").unwrap();

        copy_dir(&src, &dst).unwrap();
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

        let err = copy_dir(&src, &dst).unwrap_err();
        assert!(format!("{:#}", err).contains("symlink"));
    }
}
