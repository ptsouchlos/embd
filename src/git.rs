//! This module abstracts the git functionality needed to make `embd` work so that
//! the tool can continue to work whether a global git executable is available or not.

pub(crate) mod cli {
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result, bail};

    pub(crate) fn clone(link: &str, dest: &Path, shallow: bool) -> Result<()> {
        let mut cmd = std::process::Command::new("git");
        cmd.args(["clone", link, &dest.to_string_lossy()]);
        if shallow {
            cmd.args(["--depth", "1"]);
        }

        let out = cmd.output().context("failed to run git clone")?;
        if !out.status.success() {
            bail!(
                "git clone failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }

    pub(crate) fn checkout(repo_dir: &Path, git_tag: String) -> Result<()> {
        let out = std::process::Command::new("git")
            .current_dir(repo_dir)
            .args(["checkout", git_tag.as_str()])
            .output()
            .context("git checkout failed")?;
        if !out.status.success() {
            bail!(
                "git checkout failed:\n{}",
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
