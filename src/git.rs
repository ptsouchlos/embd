//! This module abstracts the git functionality needed to make `embd` work so that
//! the tool can continue to work whether a global git executable is available or not.

pub(crate) mod cli {
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result, bail};

    pub(crate) fn clone(link: &str, dest: &Path) -> Result<()> {
        let out = std::process::Command::new("git")
            .args(["clone", "--depth", "1", link, &dest.to_string_lossy()])
            .output()
            .context("failed to run git clone")?;
        if !out.status.success() {
            bail!(
                "git clone failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }

    pub(crate) fn commit_hash_of(repo_path: &Path) -> Result<String> {
        let out = std::process::Command::new("git")
            .args(["-C", &repo_path.to_string_lossy(), "rev-parse", "HEAD"])
            .output()
            .context("failed to run git rev-parse")?;
        if !out.status.success() {
            bail!(
                "git rev-parse failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(String::from_utf8(out.stdout)?.trim().to_string())
    }

    pub(crate) fn find_git_root(cwd: PathBuf) -> Result<PathBuf> {
        let out = std::process::Command::new("git")
            .current_dir(cwd)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .context("failed to run git")?;
        if !out.status.success() {
            bail!("not inside a git repository");
        }
        Ok(PathBuf::from(String::from_utf8(out.stdout)?.trim()))
    }
}
