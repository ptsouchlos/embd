//! This module abstracts the git functionality needed to make `embd` work so that
//! the tool can continue to work whether a global git executable is available or not.
//! Note that currently, git is required.

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
    validate_repo_name(name)
        .with_context(|| format!("could not extract a valid repo name from {trimmed}"))?;

    Ok((trimmed.to_string(), name.to_string()))
}

/// Validate that a repository name is safe to use as a filename and config key.
///
/// Accepts `[A-Za-z0-9_][A-Za-z0-9._-]*`. This keeps the name suitable for use
/// as a manifest filename across platforms and avoids shell/path footguns like
/// names beginning with `-` or `.`.
fn validate_repo_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("repo name is empty");
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphanumeric() || first == '_') {
        bail!("repo name '{name}' must start with an ASCII letter, digit, or underscore");
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
            bail!(
                "repo name '{name}' contains an unsupported character '{c}' (allowed: letters, digits, '_', '-', '.')"
            );
        }
    }
    Ok(())
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

    use crate::config::Tracking;

    /// Clone a repository at the given link to the given destination folder.
    ///
    /// # Arguments
    ///
    /// - `link`: The repository to clone, must be a valid URL (HTTPS or SSH)
    /// - `dest`: Where to clone the repo to. Must be a valid path.
    /// - `shallow`: If true, will pass `--depth 1` to `git clone` to avoid cloning all of the git history.
    /// - `branch`: If set, pass `--branch <branch>` to `git clone` so the clone
    ///   checks out that branch instead of the remote's default branch.
    ///
    /// # Returns
    ///
    /// Error if there was a failure, () otherwise.
    pub(crate) fn clone(
        link: &str,
        dest: &Path,
        shallow: bool,
        branch: Option<&str>,
    ) -> Result<()> {
        let mut cmd = std::process::Command::new("git");
        cmd.args(["clone", link, &dest.to_string_lossy()]);
        if shallow {
            cmd.args(["--depth", "1"]);
        }
        if let Some(branch) = branch {
            cmd.args(["--branch", branch]);
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

    /// Query the remote for the latest commit of a branch without cloning.
    ///
    /// # Returns
    ///
    /// - `Ok(Some(sha))`: the remote branch tip.
    /// - `Ok(None)`: the branch does not exist on the remote (or the remote
    ///   has no branches at all).
    /// - `Err`: a failure to run git or reach the remote.
    pub(crate) fn ls_remote_head(remote: &str, branch: &str) -> Result<Option<String>> {
        let spec = format!("refs/heads/{branch}");
        let out = std::process::Command::new("git")
            .args(["ls-remote", remote, &spec])
            .output()
            .context("failed to run git ls-remote")?;
        if !out.status.success() {
            bail!(
                "git ls-remote failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        // ls-remote patterns match by suffix, so the pattern can also match a
        // longer ref (e.g. `refs/heads/main` against `refs/heads/x/refs/heads/main`).
        // Only accept the exact ref we asked for.
        for line in stdout.lines() {
            let mut fields = line.split_whitespace();
            let (sha, ref_name) = match (fields.next(), fields.next()) {
                (Some(sha), Some(ref_name)) => (sha, ref_name),
                _ => continue,
            };
            if ref_name == spec.as_str() {
                return Ok(Some(sha.to_string()));
            }
        }
        Ok(None)
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

    /// Run a probe command in the repo and return its trimmed stdout.
    ///
    /// A non-zero exit means "not this kind" and yields `Ok(None)`; only a
    /// failure to run git is an `Err`.
    fn run_ok(repo_path: &Path, args: &[&str]) -> Result<Option<String>> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo_path)
            .args(args)
            .output()
            .context("failed to run git")?;
        if !out.status.success() {
            return Ok(None);
        }
        Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ))
    }

    /// Classify what the repo's HEAD resolved to after checkout, so `add` can
    /// record where the pinned commit came from instead of parsing the user's
    /// string.
    ///
    /// # Arguments
    ///
    /// - `repo_path`: Full path to the repo, already checked out.
    /// - `rev`: The ref the caller checked out, if any. When several tags point
    ///   at the same commit, this one is preferred.
    ///
    /// # Returns
    ///
    /// `Ok(Some(Branch(_)))` for an attached HEAD, `Ok(Some(Tag(_)))` for a
    /// tag, `Ok(None)` for a raw commit. A non-zero exit from a probe means
    /// "not this kind", not an error; only a failure to run git is an `Err`.
    pub(crate) fn classify_head(repo_path: &Path, rev: Option<&str>) -> Result<Option<Tracking>> {
        // Attached HEAD: a default-branch clone, or `checkout <branch>` in a
        // fresh clone (which creates a local tracking branch). Checked before
        // the tag probes on purpose: if the rev names both a branch and a
        // tag, `git checkout` resolves to the branch, so we record the
        // branch-tracking too.
        if let Some(branch) = run_ok(repo_path, &["symbolic-ref", "-q", "--short", "HEAD"])? {
            return Ok(Some(Tracking::Branch(branch)));
        }
        // Detached at the tag the caller named, if they named one.
        if let Some(rev) = rev {
            let ref_name = format!("refs/tags/{rev}");
            if run_ok(repo_path, &["show-ref", "--verify", "--quiet", &ref_name])?.is_some() {
                return Ok(Some(Tracking::Tag(rev.to_string())));
            }
        }
        // Detached at some other exact tag on HEAD.
        if let Some(tag) = run_ok(repo_path, &["describe", "--exact-match", "--tags", "HEAD"])? {
            return Ok(Some(Tracking::Tag(tag)));
        }
        Ok(None)
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

/// Shared helpers for tests that need a real git repo. Shared with the
/// `update` command's tests, hence `pub(crate)`.
#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    /// Run `git <args>` in `dir`, panicking on non-zero exit.
    pub(crate) fn git(dir: &Path, args: &[&str]) {
        git_stdout(dir, args);
    }

    /// Run `git <args>` in `dir`, panicking on non-zero exit, and return the
    /// trimmed stdout.
    pub(crate) fn git_stdout(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("failed to run git: {e}"));
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Build a throwaway "upstream" repo:
    /// - `main`: one commit (`a.txt`), tagged `v1.0`
    /// - `feature`: one more commit on top of it (`b.txt`)
    ///
    /// The repo is left checked out on `main`. The `TempDir` is returned so
    /// the caller can keep it alive.
    pub(crate) fn test_repo() -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("failed to create tempdir");
        let repo = dir.path().to_path_buf();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "embd tests"]);
        git(&repo, &["config", "user.email", "embd@example.com"]);
        std::fs::write(repo.join("a.txt"), "alpha").expect("failed to write a.txt");
        git(&repo, &["add", "a.txt"]);
        git(&repo, &["commit", "-q", "-m", "commit 1"]);
        git(&repo, &["tag", "v1.0"]);
        git(&repo, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.join("b.txt"), "beta").expect("failed to write b.txt");
        git(&repo, &["add", "b.txt"]);
        git(&repo, &["commit", "-q", "-m", "commit 2"]);
        git(&repo, &["checkout", "-q", "main"]);
        (dir, repo)
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{git, git_stdout, test_repo};
    use super::*;
    use std::path::{Path, PathBuf};
    use tempfile::{TempDir, tempdir};

    use crate::config::Tracking;

    /// Clone the upstream repo locally (full history, like a `--rev` add
    /// would) and keep both the tempdir and the clone path alive.
    fn clone_of(repo: &Path) -> (TempDir, PathBuf) {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();
        cli::clone(&repo.to_string_lossy(), &path, false, None).unwrap();
        (dir, path)
    }

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

    #[test]
    fn rejects_name_starting_with_dash() {
        assert!(parse_repo_link("git@github.com:org/-rf.git").is_err());
    }

    #[test]
    fn rejects_name_starting_with_dot() {
        assert!(parse_repo_link("git@github.com:org/.hidden.git").is_err());
    }

    #[test]
    fn rejects_name_with_space() {
        // URL parsing escapes the space, but the SSH path lets one through.
        assert!(parse_repo_link("git@github.com:org/foo bar.git").is_err());
    }

    #[test]
    fn accepts_name_with_dots_and_dashes_inside() {
        let (_, name) = parse_repo_link("https://github.com/org/foo.bar-baz.git").unwrap();
        assert_eq!(name, "foo.bar-baz");
    }

    #[test]
    fn classify_default_clone_is_branch() {
        let (_repo_dir, repo) = test_repo();
        let (_clone_dir, clone) = clone_of(&repo);
        assert_eq!(
            cli::classify_head(&clone, None).unwrap(),
            Some(Tracking::Branch("main".to_string()))
        );
    }

    #[test]
    fn classify_checked_out_branch() {
        let (_repo_dir, repo) = test_repo();
        let (_clone_dir, clone) = clone_of(&repo);
        git(&clone, &["checkout", "-q", "feature"]);
        assert_eq!(
            cli::classify_head(&clone, Some("feature")).unwrap(),
            Some(Tracking::Branch("feature".to_string()))
        );
    }

    #[test]
    fn classify_checked_out_tag() {
        let (_repo_dir, repo) = test_repo();
        let (_clone_dir, clone) = clone_of(&repo);
        git(&clone, &["checkout", "-q", "v1.0"]);
        assert_eq!(
            cli::classify_head(&clone, Some("v1.0")).unwrap(),
            Some(Tracking::Tag("v1.0".to_string()))
        );
    }

    #[test]
    fn classify_raw_commit_is_none() {
        let (_repo_dir, repo) = test_repo();
        let (_clone_dir, clone) = clone_of(&repo);
        let sha = git_stdout(&repo, &["rev-parse", "feature"]);
        git(&clone, &["checkout", "-q", sha.as_str()]);
        assert_eq!(
            cli::classify_head(&clone, Some(sha.as_str())).unwrap(),
            None
        );
    }

    #[test]
    fn classify_tag_not_named_by_rev() {
        let (_repo_dir, repo) = test_repo();
        git(&repo, &["tag", "alt"]);
        let (_clone_dir, clone) = clone_of(&repo);
        git(&clone, &["checkout", "-q", "alt"]);
        assert_eq!(
            cli::classify_head(&clone, Some("alt")).unwrap(),
            Some(Tracking::Tag("alt".to_string()))
        );
    }

    #[test]
    fn clone_with_branch_checks_out_that_branch() {
        let (_repo_dir, repo) = test_repo();
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();
        cli::clone(&repo.to_string_lossy(), &path, false, Some("feature")).unwrap();
        assert_eq!(
            cli::classify_head(&path, None).unwrap(),
            Some(Tracking::Branch("feature".to_string()))
        );
    }

    #[test]
    fn ls_remote_head_returns_branch_tip() {
        let (_repo_dir, repo) = test_repo();
        let remote = repo.to_string_lossy().to_string();
        assert_eq!(
            cli::ls_remote_head(&remote, "main").unwrap(),
            Some(cli::commit_hash_of(repo.as_path()).unwrap())
        );
    }

    #[test]
    fn ls_remote_head_ignores_suffix_matching_refs() {
        let (_repo_dir, repo) = test_repo();
        // The suffix pattern for `refs/heads/alt` also matches this longer
        // ref, which must not be treated as the branch itself.
        git(&repo, &["branch", "x/refs/heads/alt"]);
        let remote = repo.to_string_lossy().to_string();
        assert!(cli::ls_remote_head(&remote, "alt").unwrap().is_none());
    }

    #[test]
    fn ls_remote_head_missing_branch_is_none() {
        let (_repo_dir, repo) = test_repo();
        let remote = repo.to_string_lossy().to_string();
        assert!(
            cli::ls_remote_head(&remote, "no-such-branch")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn ls_remote_head_bogus_remote_is_err() {
        assert!(cli::ls_remote_head("not-a-remote-url", "main").is_err());
    }
}
