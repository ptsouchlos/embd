//! End-to-end tests that drive the compiled `embd` binary against throwaway
//! local git repos: branch/tag tracking, `add --branch`, `update --remote`
//! and the tracking suffix in `status`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("failed to run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit_file(repo: &Path, name: &str, contents: &str, msg: &str) {
    fs::write(repo.join(name), contents).unwrap();
    git(repo, &["add", name]);
    git(repo, &["commit", "-q", "-m", msg]);
}

/// An upstream repo and a host repo to embed it into.
///
/// Upstream layout:
/// - `main`: `c1` (`a.txt`, tagged `v1.0`), then `c2` (`b.txt`)
/// - `feature`: `main` plus `c3` (`c.txt`)
struct Env {
    _tmp: TempDir,
    upstream: PathBuf,
    host: PathBuf,
    home: PathBuf,
    c1: String,
    c2: String,
    c3: String,
}

impl Env {
    fn new() -> Env {
        let tmp = TempDir::new().unwrap();
        let upstream = tmp.path().join("up");
        let host = tmp.path().join("host");
        let home = tmp.path().join("home");
        for dir in [&upstream, &host, &home] {
            fs::create_dir(dir).unwrap();
        }

        git(&upstream, &["init", "-q", "-b", "main"]);
        git(&upstream, &["config", "user.name", "embd tests"]);
        git(&upstream, &["config", "user.email", "embd@example.com"]);
        commit_file(&upstream, "a.txt", "alpha", "c1");
        git(&upstream, &["tag", "v1.0"]);
        let c1 = git(&upstream, &["rev-parse", "HEAD"]);
        commit_file(&upstream, "b.txt", "beta", "c2");
        let c2 = git(&upstream, &["rev-parse", "HEAD"]);
        git(&upstream, &["checkout", "-q", "-b", "feature"]);
        commit_file(&upstream, "c.txt", "gamma", "c3");
        let c3 = git(&upstream, &["rev-parse", "HEAD"]);
        git(&upstream, &["checkout", "-q", "main"]);

        git(&host, &["init", "-q", "-b", "main"]);
        // Run embd from the path git reports for the repo. On Windows the temp
        // dir can be an 8.3 short path (`RUNNER~1`) while git reports the long
        // form, and embd compares the working directory to git's root.
        let host = PathBuf::from(git(&host, &["rev-parse", "--show-toplevel"]));

        Env {
            _tmp: tmp,
            upstream,
            host,
            home,
            c1,
            c2,
            c3,
        }
    }

    fn url(&self) -> String {
        format!("file://{}", self.upstream.display())
    }

    /// Run `embd <args>` in the host repo with an isolated HOME and no color.
    fn embd(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_embd"))
            .current_dir(&self.host)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("NO_COLOR", "1")
            .args(args)
            .output()
            .expect("failed to run embd")
    }

    /// `embd add <url> <folder> <extra...>`.
    fn add(&self, folder: &str, extra: &[&str]) -> Output {
        let url = self.url();
        let mut args = vec!["add", url.as_str(), folder];
        args.extend_from_slice(extra);
        self.embd(&args)
    }

    fn record(&self, folder: &str) -> String {
        fs::read_to_string(self.host.join(folder).join(".embd")).unwrap()
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("terminated by signal")
}

#[test]
fn add_branch_requires_rev() {
    let env = Env::new();
    let out = env.add("e", &["--branch", "main"]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(stderr(&out).contains("--rev"), "{}", stderr(&out));
    assert!(!env.host.join("e").exists());
}

#[test]
fn add_branch_with_ancestor_pin_records_branch_tracking() {
    let env = Env::new();
    let out = env.add("e", &["--rev", &env.c1, "--branch", "main"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));

    let record = env.record("e");
    assert!(record.contains(&format!("commit_hash = \"{}\"", env.c1)));
    assert!(record.contains("[metadata.tracking]"), "{record}");
    assert!(record.contains("branch = \"main\""), "{record}");
    assert!(env.host.join("e/a.txt").exists());
    assert!(!env.host.join("e/b.txt").exists());
}

#[test]
fn add_branch_rejects_unreachable_pin_and_leaves_nothing() {
    let env = Env::new();
    // c3 lives only on `feature`, so it is not reachable from `main`.
    let out = env.add("e", &["--rev", &env.c3, "--branch", "main"]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("not reachable"), "{}", stderr(&out));
    assert!(!env.host.join("e").exists());
}

#[test]
fn add_branch_rejects_missing_branch_and_leaves_nothing() {
    let env = Env::new();
    let out = env.add("e", &["--rev", "v1.0", "--branch", "nope"]);
    assert_eq!(code(&out), 1);
    assert!(
        stderr(&out).contains("does not exist on the remote"),
        "{}",
        stderr(&out)
    );
    assert!(!env.host.join("e").exists());
}

#[test]
fn add_without_rev_tracks_the_default_branch() {
    let env = Env::new();
    assert_eq!(code(&env.add("e", &[])), 0);
    assert!(env.record("e").contains("branch = \"main\""));
}

#[test]
fn add_rev_branch_name_tracks_that_branch() {
    let env = Env::new();
    assert_eq!(code(&env.add("e", &["--rev", "feature"])), 0);
    let record = env.record("e");
    assert!(record.contains("branch = \"feature\""), "{record}");
    assert!(record.contains(&format!("commit_hash = \"{}\"", env.c3)));
}

#[test]
fn add_rev_tag_records_tag_tracking() {
    let env = Env::new();
    assert_eq!(code(&env.add("e", &["--rev", "v1.0"])), 0);
    assert!(env.record("e").contains("tag = \"v1.0\""));
}

#[test]
fn add_rev_raw_hash_records_no_tracking() {
    let env = Env::new();
    assert_eq!(code(&env.add("e", &["--rev", &env.c2])), 0);
    assert!(!env.record("e").contains("tracking"));
}

#[test]
fn status_shows_tracking_suffix_for_each_kind() {
    let env = Env::new();
    assert_eq!(code(&env.add("branch", &["--rev", "feature"])), 0);
    assert_eq!(code(&env.add("tag", &["--rev", "v1.0"])), 0);
    assert_eq!(code(&env.add("raw", &["--rev", &env.c2])), 0);

    let out = env.embd(&["status"]);
    assert_eq!(code(&out), 0, "{}", stdout(&out));
    let text = stdout(&out);
    assert!(text.contains("branch (tracking feature): clean"), "{text}");
    assert!(text.contains("tag (tag v1.0): clean"), "{text}");
    assert!(text.contains("raw: clean"), "{text}");
}

#[test]
fn status_reports_drift_on_a_tracked_embed() {
    let env = Env::new();
    assert_eq!(code(&env.add("e", &[])), 0);
    fs::remove_file(env.host.join("e/a.txt")).unwrap();
    fs::write(env.host.join("e/b.txt"), "changed").unwrap();

    let out = env.embd(&["status"]);
    assert_eq!(code(&out), 1, "{}", stdout(&out));
    let text = stdout(&out);
    assert!(text.contains("e (tracking main): drift"), "{text}");
    assert!(text.contains("D  a.txt"), "{text}");
    assert!(text.contains("M  b.txt"), "{text}");
}

#[test]
fn update_remote_advances_a_branch_tracking_embed() {
    let env = Env::new();
    assert_eq!(
        code(&env.add("e", &["--rev", &env.c1, "--branch", "main"])),
        0
    );

    let out = env.embd(&["update", "--remote"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&out).contains("updated"), "{}", stdout(&out));
    assert!(env.host.join("e/b.txt").exists());

    let record = env.record("e");
    assert!(record.contains(&format!("commit_hash = \"{}\"", env.c2)));
    assert!(record.contains("branch = \"main\""), "{record}");
    assert_eq!(code(&env.embd(&["status"])), 0);
}

#[test]
fn update_remote_is_a_no_op_when_already_at_the_tip() {
    let env = Env::new();
    assert_eq!(code(&env.add("e", &[])), 0);
    let before = env.record("e");

    let out = env.embd(&["update", "--remote"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(env.record("e"), before);
}

#[test]
fn update_remote_leaves_tag_pinned_embeds_alone() {
    let env = Env::new();
    assert_eq!(code(&env.add("e", &["--rev", "v1.0"])), 0);
    let before = env.record("e");

    let out = env.embd(&["update", "--remote"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("pinned to tag v1.0"),
        "{}",
        stdout(&out)
    );
    assert_eq!(env.record("e"), before);
}

#[test]
fn update_remote_reports_entries_without_a_tracking_ref() {
    let env = Env::new();
    assert_eq!(code(&env.add("e", &["--rev", &env.c2])), 0);

    let out = env.embd(&["update", "--remote"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(stdout(&out).contains("no tracking ref"), "{}", stdout(&out));
}

#[test]
fn update_remote_conflicts_with_rev() {
    let env = Env::new();
    let out = env.embd(&["update", "--remote", "--rev", "v1.0"]);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("cannot be used with"),
        "{}",
        stderr(&out)
    );
}
