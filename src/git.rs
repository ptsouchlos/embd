//! This module abstracts the git functionality needed to make `embd` work so that
//! the tool can continue to work whether a global git executable is available or not.

use anyhow::{Context, Result, bail};
use url::Url;

/// Parse a repository link and extract a name for it.
///
/// Accepts both standard URLs (`https://github.com/org/repo.git`) and
/// SSH refs (`git@github.com:org/repo.git`). Returns the trimmed link and the
/// repo name (last path segment with any trailing `.git` removed).
///
/// # Arguments
///
/// - `link`: The link to parse the name from.
///
/// # Returns
///
/// The trimmed, original link and the repo name.
pub(crate) fn parse_repo_link(link: &str) -> Result<(String, String)> {
    let trimmed = link.trim();
    if trimmed.is_empty() {
        bail!("repository link is empty");
    }

    let path = if let Some(scp_path) = parse_ssh_style_path(trimmed) {
        scp_path.to_string()
    } else {
        let url =
            Url::parse(trimmed).with_context(|| format!("invalid repository link: {trimmed}"))?;
        url.path().to_string()
    };

    let last = path
        .trim_end_matches('/')
        .rsplit('/')
        .find(|s| !s.is_empty())
        .with_context(|| format!("could not extract repo name from {trimmed}"))?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    if name.is_empty() {
        bail!("could not extract repo name from {trimmed}");
    }

    Ok((trimmed.to_string(), name.to_string()))
}

/// Recognize scp-style SSH refs like `git@host:org/repo.git` and return the
/// path portion after the colon. Returns None for anything that looks like a
/// URL (contains `://`) or doesn't match the pattern.
///
/// # Arguments
///
/// - `link`: The link to parse.
///
/// # Returns
///
/// Optional string of the repo name as parsed from the link.
fn parse_ssh_style_path(link: &str) -> Option<&str> {
    if link.contains("://") {
        return None;
    }
    let (before, after) = link.split_once(':')?;
    if before.is_empty() || after.is_empty() {
        return None;
    }
    // Avoid mistaking a Windows drive path like `C:\foo` for SSH.
    if before.len() == 1 && before.chars().next()?.is_ascii_alphabetic() {
        return None;
    }
    Some(after)
}

pub(crate) mod cli {
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result, bail};

    /// Clone a repository at the given link to the given destination folder.
    ///
    /// # Arguments
    ///
    /// - `link`: The repository to clone, must be a valid URL (HTTPS or SSH)
    /// - `dest`: Where to clone the repo to. Must be a valid path.
    /// - `shallow`: If true, will pass `--depth 1` to `git clone` to avoid cloning all of the git history.
    ///
    /// # Returns
    ///
    /// Error if there was a failure, () otherwise.
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

    /// Checkout the given tag in the given repository directory.
    ///
    /// # Arguments
    ///
    /// - `repo_dir`: Full path to the git repo.
    /// - `git_tag`: Tag to checkout.
    ///
    /// # Returns
    ///
    /// Error if checkout fails, () otherwise.
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

    /// Get the commit hash of the repo at the given path.
    ///
    /// # Arguments
    ///
    /// - `repo_path`: Full path to the repo.
    ///
    /// # Returns
    ///
    /// Commit hash of the repo's current state, error otherwise.
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

    /// Find the root of a git repo.
    ///
    /// # Arguments
    ///
    /// - `cwd`: The current working directory somewhere inside a git repo.
    ///
    /// # Returns
    ///
    /// The root directory if found, error otherwise.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_https_url() {
        let (link, name) = parse_repo_link("https://github.com/org/repo.git").unwrap();
        assert_eq!(link, "https://github.com/org/repo.git");
        assert_eq!(name, "repo");
    }

    #[test]
    fn parse_https_url_no_dot_git() {
        let (_, name) = parse_repo_link("https://github.com/org/repo").unwrap();
        assert_eq!(name, "repo");
    }

    #[test]
    fn parse_https_url_trailing_slash() {
        let (_, name) = parse_repo_link("https://github.com/org/repo/").unwrap();
        assert_eq!(name, "repo");
    }

    #[test]
    fn parse_ssh_scp_style() {
        let (link, name) = parse_repo_link("git@github.com:org/repo.git").unwrap();
        assert_eq!(link, "git@github.com:org/repo.git");
        assert_eq!(name, "repo");
    }

    #[test]
    fn parse_ssh_url_style() {
        let (_, name) = parse_repo_link("ssh://git@github.com/org/repo.git").unwrap();
        assert_eq!(name, "repo");
    }

    #[test]
    fn parse_empty_fails() {
        assert!(parse_repo_link("").is_err());
        assert!(parse_repo_link("   ").is_err());
    }

    #[test]
    fn parse_no_path_fails() {
        assert!(parse_repo_link("https://github.com").is_err());
        assert!(parse_repo_link("https://github.com/").is_err());
    }

    #[test]
    fn parse_garbage_fails() {
        assert!(parse_repo_link("not a url").is_err());
    }
}
