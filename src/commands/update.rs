use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tempfile::tempdir;

use crate::color;
use crate::commands::common::select_entries;
use crate::commands::status::print_report;
use crate::config::{self, EmbdEntry, EntryState, FileChange, FileLocks, Tracking};
use crate::filter::Filter;
use crate::{git, paths};

#[derive(clap::Args, Debug)]
pub(crate) struct UpdateArgs {
    /// Optional folder paths to update. When empty, every embed in the project is updated.
    folders: Vec<String>,
    /// Rev (commit hash, tag, or branch) to advance the entry's pin to.
    /// Requires exactly one folder.
    #[clap(short, long)]
    rev: Option<String>,
    /// Advance branch-tracking embeds to the latest commit on their tracked branch.
    #[clap(long, conflicts_with = "rev")]
    remote: bool,
    /// Overwrite local modifications to tracked files. Untracked files are
    /// preserved by default; combine with --overwrite to remove them too.
    #[clap(long)]
    force: bool,
    /// Delete files on disk that are not in the new pinned tree (including
    /// untracked extras). Requires --force.
    #[clap(long)]
    overwrite: bool,
    /// Suppress per-file rows; print only one-line summaries per entry.
    #[clap(short, long)]
    quiet: bool,
}

pub(crate) fn execute(args: UpdateArgs) -> Result<()> {
    if args.overwrite && !args.force {
        bail!("--overwrite requires --force");
    }

    let root = paths::find_git_root()?;
    let cwd = std::env::current_dir().context("failed to read current directory")?;
    let selected = select_entries(&root, &args.folders, &cwd)?;

    if args.rev.is_some() && args.folders.len() != 1 {
        bail!("--rev requires exactly one folder to be specified");
    }

    let mut any_failed = false;
    for (folder, entry) in selected {
        match process_entry(&root, &folder, entry, &args) {
            Ok(outcome) => {
                print_outcome(&folder, &outcome, args.quiet);
                if outcome.is_failure() {
                    any_failed = true;
                }
            }
            Err(e) => {
                anstream::eprintln!("{} {}: {e:#}", color::error_label(), folder.display());
                any_failed = true;
            }
        }
    }

    if any_failed {
        std::process::exit(1);
    }
    Ok(())
}

/// Per-entry outcome.
#[derive(Debug)]
enum Outcome {
    /// Clean, no `--rev` (or rev resolves to the existing pin). No clone done.
    UpToDate,
    /// Drift detected, `--force` not given. Drift was already printed via `print_report`.
    SkippedDrift,
    /// Applied: files synced, config saved (pin + manifest) if anything changed.
    Updated {
        old_commit: String,
        new_commit: String,
        changes: Vec<UpdateChange>,
    },
    /// The entry is pinned to a tag; `--remote` does not advance it.
    PinnedToTag(String),
    /// The entry has no tracking ref, so `--remote` cannot advance it.
    NoTracking,
}

impl Outcome {
    fn is_failure(&self) -> bool {
        matches!(self, Outcome::SkippedDrift)
    }
}

/// What `process_entry` is doing with this entry, resolved up front.
enum Target<'a> {
    /// Apply the pinned revision; the pin does not move.
    Resync(&'a str),
    /// Advance the pin to a named revision (may move the pin).
    Rev(&'a str),
    /// Advance the pin to the latest commit on the tracked branch.
    Branch(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
enum UpdateChange {
    Wrote(String),
    Deleted(String),
    Removed(String), // --overwrite swept this
}

impl UpdateChange {
    /// Helper to convert [`UpdateChange`] to a marker (char).
    pub(crate) fn as_marker(&self) -> char {
        match self {
            Self::Wrote(_) => 'W',
            Self::Deleted(_) => 'D',
            Self::Removed(_) => 'X',
        }
    }
}

fn process_entry(
    root: &Path,
    folder: &Path,
    mut entry: EmbdEntry,
    args: &UpdateArgs,
) -> Result<Outcome> {
    let config_path = paths::embed_file_path(&root.join(folder));
    let report = config::inspect_entry(root, folder, &entry);

    // Resolve what this run is doing before touching the network or disk.
    let target: Target = if args.remote {
        match &entry.metadata.tracking {
            Some(Tracking::Tag(tag)) => return Ok(Outcome::PinnedToTag(tag.clone())),
            Some(Tracking::Branch(branch)) => Target::Branch(branch),
            None => return Ok(Outcome::NoTracking),
        }
    } else if let Some(rev) = &args.rev {
        Target::Rev(rev)
    } else {
        Target::Resync(&entry.metadata.commit_hash)
    };
    let moves_pin = !matches!(target, Target::Resync(_));

    // Short-circuit no-op: only safe when the pin does not move. With a pin
    // bump we must still resolve the ref to know whether it changes the pin.
    if !moves_pin && report.state == EntryState::Compared && report.changes.is_empty() {
        return Ok(Outcome::UpToDate);
    }

    // Drift gate. Only file-level drift requires --force; a pure --rev bump is
    // what update exists to apply.
    let has_file_drift = report.state == EntryState::Compared
        && report.changes.iter().any(|c| match c {
            FileChange::Untracked(_) => !entry.metadata.allow_untracked,
            _ => true,
        });
    if has_file_drift && !args.force {
        print_report(&report, args.quiet);
        return Ok(Outcome::SkippedDrift);
    }

    // Clone the target rev to a tempdir and resolve it to a concrete commit.
    let tmp = tempdir().context("failed to create temporary directory")?;
    let (new_commit, tracking) = match target {
        Target::Branch(branch) => {
            // Ask upstream where the tracked branch currently points.
            let head = match git::cli::ls_remote_head(&entry.metadata.remote, branch)? {
                Some(head) => head,
                None => bail!(
                    "branch '{}' is gone upstream on {}; re-pin with `embd update {} --rev <ref>`",
                    branch,
                    entry.metadata.remote,
                    folder.display()
                ),
            };
            // Fast path: the branch tip is the pin itself and the tree is clean,
            // so nothing has moved. Skip the clone.
            if head == entry.metadata.commit_hash
                && report.state == EntryState::Compared
                && report.changes.is_empty()
            {
                return Ok(Outcome::UpToDate);
            }
            // Shallow clone straight at the branch tip: no checkout, no re-classify.
            git::cli::clone(&entry.metadata.remote, tmp.path(), true, Some(branch))?;
            (
                git::cli::commit_hash_of(tmp.path())?,
                Some(Tracking::Branch(branch.to_string())),
            )
        }
        Target::Resync(rev) | Target::Rev(rev) => {
            // Always full clone: the rev checked out may be any historical
            // commit (the pin for re-sync, or a named rev for a bump).
            git::cli::clone(&entry.metadata.remote, tmp.path(), false, None)?;
            git::cli::checkout(tmp.path(), rev.to_string())?;
            // Re-classify only when the pin moves; otherwise keep what we recorded.
            let tracking = if moves_pin {
                git::cli::classify_head(tmp.path(), Some(rev))?
            } else {
                entry.metadata.tracking.clone()
            };
            (git::cli::commit_hash_of(tmp.path())?, tracking)
        }
    };

    // If a bump resolved back to the existing pin with a clean tree and the
    // tracking is unchanged, no-op. Tracking may legitimately change on its
    // own (re-pinning a raw-hash entry to a branch that already holds it).
    let tracking_changed = tracking != entry.metadata.tracking;
    if moves_pin
        && new_commit == entry.metadata.commit_hash
        && !tracking_changed
        && report.state == EntryState::Compared
        && report.changes.is_empty()
    {
        return Ok(Outcome::UpToDate);
    }

    // Folder-missing recovery: create the destination folder. Lossless.
    let folder_abs = root.join(folder);
    if report.state == EntryState::FolderMissing {
        std::fs::create_dir_all(&folder_abs)
            .with_context(|| format!("failed to create folder {}", folder_abs.display()))?;
    }

    let old_commit = entry.metadata.commit_hash.clone();

    // Save config first when the pin or the tracking moves, before the file
    // manifest catches up. Durable pin is the source of truth; a crash after
    // this point is recoverable via `update --force`.
    if moves_pin && (new_commit != entry.metadata.commit_hash || tracking_changed) {
        entry.metadata.commit_hash = new_commit.clone();
        entry.metadata.tracking = tracking;
        entry.save(&config_path)?;
    }

    // Build the prospective file manifest from the temp clone, applying the
    // same include/exclude filter the entry was added with so filtered-out
    // files are never re-introduced.
    let filter = Filter::from_patterns(&entry.metadata.include, &entry.metadata.exclude)?;
    let new_files = FileLocks::build_from_path_filtered(tmp.path(), &filter)?;

    // Take the old file list from the entry's current manifest, or treat as
    // empty for folder-missing.
    let old_files: BTreeMap<String, String> = match report.state {
        EntryState::Compared => entry.files.as_map().clone(),
        EntryState::FolderMissing => BTreeMap::new(),
    };

    let changes = apply_update(
        tmp.path(),
        &folder_abs,
        &old_files,
        new_files.as_map(),
        args.overwrite,
    )?;

    // Save the updated file manifest last.
    entry.files = new_files;
    entry.save(&config_path)?;

    Ok(Outcome::Updated {
        old_commit,
        new_commit,
        changes,
    })
}

/// Pure-file-ops engine: brings `dst` into agreement with `new_files` (taken
/// from `src`), using `old_files` as the previous manifest. Writes that match
/// what's already on disk are skipped (idempotent). `overwrite` also deletes
/// any on-disk file that isn't in `new_files`.
fn apply_update(
    src: &Path,
    dst: &Path,
    old_files: &BTreeMap<String, String>,
    new_files: &BTreeMap<String, String>,
    overwrite: bool,
) -> Result<Vec<UpdateChange>> {
    let mut changes = Vec::new();
    let mut emptied_dirs: BTreeSet<PathBuf> = BTreeSet::new();

    // Writes: entries in `new_files`. Skip if the destination already matches.
    for (key, new_hash) in new_files {
        let dst_path = dst.join(key_to_path(key));

        if let Ok(meta) = std::fs::symlink_metadata(&dst_path)
            && meta.is_file()
            && let Ok(disk_hash) = config::hash_file(&dst_path)
            && disk_hash == *new_hash
        {
            continue;
        }

        if let Some(parent) = dst_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }
        // A pre-existing regular file or symlink at this path must be removed
        // first; copy doesn't replace a symlink in-place.
        if let Ok(meta) = std::fs::symlink_metadata(&dst_path)
            && (meta.is_file() || meta.file_type().is_symlink())
        {
            std::fs::remove_file(&dst_path)
                .with_context(|| format!("failed to remove {}", dst_path.display()))?;
        }
        let src_path = src.join(key_to_path(key));
        std::fs::copy(&src_path, &dst_path).with_context(|| {
            format!(
                "failed to copy {} to {}",
                src_path.display(),
                dst_path.display()
            )
        })?;
        changes.push(UpdateChange::Wrote(key.clone()));
    }

    // Deletes: entries present in `old_files` but not in `new_files`.
    for key in old_files.keys() {
        if new_files.contains_key(key) {
            continue;
        }
        let dst_path = dst.join(key_to_path(key));
        if let Ok(meta) = std::fs::symlink_metadata(&dst_path)
            && (meta.is_file() || meta.file_type().is_symlink())
        {
            std::fs::remove_file(&dst_path)
                .with_context(|| format!("failed to remove {}", dst_path.display()))?;
            if let Some(parent) = dst_path.parent() {
                emptied_dirs.insert(parent.to_path_buf());
            }
            changes.push(UpdateChange::Deleted(key.clone()));
        }
    }

    // --overwrite sweep: remove anything on disk that isn't in `new_files`.
    if overwrite {
        let on_disk = config::scan_folder(dst).unwrap_or_default();
        for key in on_disk.keys() {
            if new_files.contains_key(key) {
                continue;
            }
            // Don't double-report files we just deleted in the previous step.
            if old_files.contains_key(key) {
                continue;
            }
            let dst_path = dst.join(key_to_path(key));
            if let Ok(meta) = std::fs::symlink_metadata(&dst_path)
                && (meta.is_file() || meta.file_type().is_symlink())
            {
                std::fs::remove_file(&dst_path)
                    .with_context(|| format!("failed to remove {}", dst_path.display()))?;
                if let Some(parent) = dst_path.parent() {
                    emptied_dirs.insert(parent.to_path_buf());
                }
                changes.push(UpdateChange::Removed(key.clone()));
            }
        }
    }

    prune_empty_dirs(dst, emptied_dirs);

    Ok(changes)
}

fn key_to_path(key: &str) -> PathBuf {
    let mut p = PathBuf::new();
    for part in key.split('/') {
        p.push(part);
    }
    p
}

fn prune_empty_dirs(root: &Path, candidates: BTreeSet<PathBuf>) {
    let mut all: Vec<PathBuf> = candidates.into_iter().collect();
    // Try the deepest dirs first so removing a leaf frees the parent.
    all.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for mut dir in all {
        loop {
            if dir == *root || !dir.starts_with(root) {
                break;
            }
            if std::fs::remove_dir(&dir).is_err() {
                break;
            }
            match dir.parent() {
                Some(parent) => dir = parent.to_path_buf(),
                None => break,
            }
        }
    }
}

fn short(commit: &str) -> &str {
    if commit.len() >= 7 {
        &commit[..7]
    } else {
        commit
    }
}

fn print_outcome(folder: &Path, outcome: &Outcome, quiet: bool) {
    use anstream::{eprintln, println};

    let header = color::header(&folder.display().to_string());
    match outcome {
        Outcome::UpToDate => println!("{header}: {}", color::ok("up to date")),
        Outcome::PinnedToTag(tag) => {
            println!(
                "{header}: {}",
                color::warn(&format!("pinned to tag {tag}; nothing to advance"))
            );
        }
        Outcome::NoTracking => {
            println!(
                "{header}: {}",
                color::warn("no tracking ref; re-pin with `embd update <folder> --rev <branch>`")
            );
        }
        Outcome::SkippedDrift => {
            // The detailed diff was already printed via print_report. Add a trailing
            // hint so the user knows what to do.
            eprintln!(
                "{} re-run with --force to overwrite local modifications",
                color::error_label()
            );
        }
        Outcome::Updated {
            old_commit,
            new_commit,
            changes,
        } => {
            let n = changes.len();
            let plural = if n == 1 { "change" } else { "changes" };
            if old_commit == new_commit {
                println!("{header}: {}, {n} {plural}", color::ok("updated"));
            } else {
                println!(
                    "{header}: {} {} -> {}, {n} {plural}",
                    color::ok("updated"),
                    color::dim(short(old_commit)),
                    color::dim(short(new_commit))
                );
            }
            if !quiet {
                for change in changes {
                    let mrkr = change.as_marker();
                    match change {
                        UpdateChange::Wrote(p) => println!("  {}  {p}", color::marker(mrkr)),
                        UpdateChange::Deleted(p) => println!("  {}  {p}", color::marker(mrkr)),
                        UpdateChange::Removed(p) => {
                            println!("  {}  {p} (untracked, removed)", color::marker(mrkr))
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::test_support::{git, git_stdout, test_repo};
    use clap::Parser;
    use std::fs;
    use tempfile::{TempDir, tempdir};

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn manifest_for(folder: &Path, _commit: &str) -> BTreeMap<String, String> {
        FileLocks::build_from_path(folder).unwrap().as_map().clone()
    }

    #[test]
    fn apply_write_creates_new_files_and_subdirs() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        write(&src.join("a.txt"), "alpha");
        write(&src.join("sub/b.txt"), "beta");
        fs::create_dir_all(&dst).unwrap();

        let new = manifest_for(&src, "new");
        let changes = apply_update(&src, &dst, &BTreeMap::new(), &new, false).unwrap();
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, UpdateChange::Wrote(k) if k == "a.txt"))
        );
        assert!(
            changes
                .iter()
                .any(|c| matches!(c, UpdateChange::Wrote(k) if k == "sub/b.txt"))
        );
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "alpha");
        assert_eq!(fs::read_to_string(dst.join("sub/b.txt")).unwrap(), "beta");
    }

    #[test]
    fn apply_skips_writes_when_disk_already_matches() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        write(&src.join("a.txt"), "alpha");
        write(&dst.join("a.txt"), "alpha");

        let new = manifest_for(&src, "new");
        let changes = apply_update(&src, &dst, &new, &new, false).unwrap();
        assert!(changes.is_empty(), "got: {changes:?}");
    }

    #[test]
    fn apply_overwrites_modified_when_forced() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        write(&src.join("a.txt"), "alpha");
        write(&dst.join("a.txt"), "MODIFIED");

        let old = manifest_for(&src, "old"); // captures the upstream hash
        let new = old.clone(); // unchanged upstream
        let changes = apply_update(&src, &dst, &old, &new, false).unwrap();
        assert_eq!(changes, vec![UpdateChange::Wrote("a.txt".into())]);
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "alpha");
    }

    #[test]
    fn apply_deletes_files_removed_from_new_tree() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        write(&src.join("kept.txt"), "k");
        write(&dst.join("kept.txt"), "k");
        write(&dst.join("gone.txt"), "g");

        let mut old = manifest_for(&src, "old");
        old.insert("gone.txt".into(), "sha256:dummy".into());
        let new = manifest_for(&src, "new");

        let changes = apply_update(&src, &dst, &old, &new, false).unwrap();
        assert_eq!(changes, vec![UpdateChange::Deleted("gone.txt".into())]);
        assert!(!dst.join("gone.txt").exists());
        assert!(dst.join("kept.txt").exists());
    }

    #[test]
    fn apply_without_overwrite_preserves_untracked() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        write(&src.join("a.txt"), "alpha");
        write(&dst.join("a.txt"), "alpha");
        write(&dst.join("extra.txt"), "x");

        let new = manifest_for(&src, "new");
        apply_update(&src, &dst, &new, &new, false).unwrap();
        assert!(dst.join("extra.txt").exists());
    }

    #[test]
    fn apply_with_overwrite_removes_untracked() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        write(&src.join("a.txt"), "alpha");
        write(&dst.join("a.txt"), "alpha");
        write(&dst.join("extra.txt"), "x");

        let new = manifest_for(&src, "new");
        let changes = apply_update(&src, &dst, &new, &new, true).unwrap();
        assert_eq!(changes, vec![UpdateChange::Removed("extra.txt".into())]);
        assert!(!dst.join("extra.txt").exists());
        assert!(dst.join("a.txt").exists());
    }

    #[test]
    fn apply_prunes_emptied_directories() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        write(&src.join("keep.txt"), "k");
        write(&dst.join("keep.txt"), "k");
        write(&dst.join("deep/sub/gone.txt"), "g");

        let mut old = manifest_for(&src, "old");
        old.insert("deep/sub/gone.txt".into(), "sha256:dummy".into());
        let new = manifest_for(&src, "new");

        apply_update(&src, &dst, &old, &new, false).unwrap();
        assert!(
            !dst.join("deep/sub").exists(),
            "empty subdir should be pruned"
        );
        assert!(
            !dst.join("deep").exists(),
            "empty parent should also be pruned"
        );
    }

    #[cfg(unix)]
    #[test]
    fn apply_replaces_symlink_at_tracked_path_when_forced() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        write(&src.join("a.txt"), "alpha");
        write(&dst.join("real"), "r");
        symlink("real", dst.join("a.txt")).unwrap();

        let old = manifest_for(&src, "old");
        let new = old.clone();
        apply_update(&src, &dst, &old, &new, false).unwrap();
        let meta = fs::symlink_metadata(dst.join("a.txt")).unwrap();
        assert!(meta.is_file(), "tracked path must be a regular file now");
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "alpha");
    }

    #[test]
    fn apply_no_lockfile_path_writes_everything_from_new_tree() {
        // Simulates --force on no-lockfile: old_files is empty.
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        write(&src.join("a.txt"), "alpha");
        write(&src.join("b.txt"), "beta");
        write(&dst.join("a.txt"), "DIFFERENT"); // existed but stale

        let new = manifest_for(&src, "new");
        let changes = apply_update(&src, &dst, &BTreeMap::new(), &new, false).unwrap();
        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "alpha");
        assert_eq!(fs::read_to_string(dst.join("b.txt")).unwrap(), "beta");
        // a.txt should be in changes (it was wrong); b.txt is new.
        let wrote_keys: Vec<&str> = changes
            .iter()
            .filter_map(|c| match c {
                UpdateChange::Wrote(k) => Some(k.as_str()),
                _ => None,
            })
            .collect();
        assert!(wrote_keys.contains(&"a.txt"));
        assert!(wrote_keys.contains(&"b.txt"));
    }

    struct Fixture {
        _repo_tmp: TempDir,
        _root_tmp: TempDir,
        repo: PathBuf,
        root: PathBuf,
    }

    impl Fixture {
        fn embedded(pin_ref: &str, tracking: Option<Tracking>, files: &[(&str, &str)]) -> Fixture {
            let (_repo_tmp, repo) = test_repo();
            let spec = format!("{pin_ref}^{{commit}}");
            let pin = git_stdout(&repo, &["rev-parse", &spec]);
            let root_tmp = tempdir().unwrap();
            let root = root_tmp.path().to_path_buf();
            let folder = root.join("myembed");
            for (name, contents) in files {
                write(&folder.join(name), contents);
            }
            let entry = EmbdEntry {
                metadata: config::Metadata {
                    remote: format!("file://{repo}", repo = repo.display()),
                    commit_hash: pin,
                    tracking,
                    allow_untracked: false,
                    include: vec![],
                    exclude: vec![],
                },
                files: FileLocks::build_from_path(&folder).unwrap(),
            };
            entry.save(&paths::embed_file_path(&folder)).unwrap();
            Fixture {
                _repo_tmp,
                _root_tmp: root_tmp,
                repo,
                root,
            }
        }

        fn folder(&self) -> PathBuf {
            self.root.join("myembed")
        }
    }

    fn args(rev: Option<&str>, remote: bool) -> UpdateArgs {
        UpdateArgs {
            folders: vec![],
            rev: rev.map(String::from),
            remote,
            force: false,
            overwrite: false,
            quiet: true,
        }
    }

    #[test]
    fn remote_conflicts_with_rev() {
        #[derive(Debug, clap::Parser)]
        struct Cli {
            #[clap(flatten)]
            update: UpdateArgs,
        }
        let ok = Cli::try_parse_from(["embd", "--remote"]).unwrap();
        assert!(ok.update.remote);
        let err = Cli::try_parse_from(["embd", "--rev", "v1.0", "--remote"]).unwrap_err();
        assert!(err.to_string().contains("cannot be used with"), "{err}");
    }

    #[test]
    fn remote_advances_branch_tracking_embed_to_branch_tip() {
        let f = Fixture::embedded(
            "main",
            Some(Tracking::Branch("main".into())),
            &[("a.txt", "alpha")],
        );
        write(&f.repo.join("c.txt"), "gamma");
        git(&f.repo, &["add", "c.txt"]);
        git(&f.repo, &["commit", "-q", "-m", "commit 3"]);
        let tip = git_stdout(&f.repo, &["rev-parse", "main^{commit}"]);

        let entry = EmbdEntry::load(&paths::embed_file_path(&f.folder())).unwrap();
        let outcome =
            process_entry(&f.root, Path::new("myembed"), entry, &args(None, true)).unwrap();

        match outcome {
            Outcome::Updated { new_commit, .. } => assert_eq!(new_commit, tip),
            other => panic!("expected Updated, got {other:?}"),
        }
        assert_eq!(
            fs::read_to_string(f.folder().join("c.txt")).unwrap(),
            "gamma"
        );
        assert_eq!(
            fs::read_to_string(f.folder().join("a.txt")).unwrap(),
            "alpha"
        );
        let saved = EmbdEntry::load(&paths::embed_file_path(&f.folder())).unwrap();
        assert_eq!(saved.metadata.commit_hash, tip);
        assert_eq!(
            saved.metadata.tracking,
            Some(Tracking::Branch("main".into()))
        );
    }

    #[test]
    fn remote_leaves_tag_pinned_embed_untouched() {
        let f = Fixture::embedded(
            "v1.0",
            Some(Tracking::Tag("v1.0".into())),
            &[("a.txt", "alpha")],
        );
        write(&f.repo.join("c.txt"), "gamma");
        git(&f.repo, &["add", "c.txt"]);
        git(&f.repo, &["commit", "-q", "-m", "commit 3"]);
        let pin = git_stdout(&f.repo, &["rev-parse", "v1.0^{commit}"]);

        let entry = EmbdEntry::load(&paths::embed_file_path(&f.folder())).unwrap();
        let outcome =
            process_entry(&f.root, Path::new("myembed"), entry, &args(None, true)).unwrap();
        assert!(matches!(outcome, Outcome::PinnedToTag(ref tag) if tag == "v1.0"));

        let saved = EmbdEntry::load(&paths::embed_file_path(&f.folder())).unwrap();
        assert_eq!(saved.metadata.commit_hash, pin);
        assert_eq!(saved.metadata.tracking, Some(Tracking::Tag("v1.0".into())));
        assert!(!f.folder().join("c.txt").exists());
    }

    #[test]
    fn remote_reports_no_tracking_entry_informationally() {
        let f = Fixture::embedded("main", None, &[("a.txt", "alpha")]);
        let mut entry = EmbdEntry::load(&paths::embed_file_path(&f.folder())).unwrap();
        entry.metadata.remote = "not-a-url".into();
        entry.save(&paths::embed_file_path(&f.folder())).unwrap();

        let outcome =
            process_entry(&f.root, Path::new("myembed"), entry, &args(None, true)).unwrap();
        assert!(matches!(outcome, Outcome::NoTracking));
    }

    #[test]
    fn remote_reports_gone_branch_as_error() {
        let f = Fixture::embedded(
            "main",
            Some(Tracking::Branch("no-such-branch".into())),
            &[("a.txt", "alpha")],
        );

        let entry = EmbdEntry::load(&paths::embed_file_path(&f.folder())).unwrap();
        let err =
            process_entry(&f.root, Path::new("myembed"), entry, &args(None, true)).unwrap_err();
        assert!(err.to_string().contains("gone upstream"), "{err}");
    }

    #[test]
    fn remote_fast_path_when_pin_matches_branch_tip() {
        let f = Fixture::embedded(
            "main",
            Some(Tracking::Branch("main".into())),
            &[("a.txt", "alpha")],
        );

        let entry = EmbdEntry::load(&paths::embed_file_path(&f.folder())).unwrap();
        let outcome =
            process_entry(&f.root, Path::new("myembed"), entry, &args(None, true)).unwrap();
        assert!(matches!(outcome, Outcome::UpToDate));
    }

    #[test]
    fn rev_bump_records_branch_tracking() {
        let f = Fixture::embedded("main", None, &[("a.txt", "alpha")]);
        let feature_tip = git_stdout(&f.repo, &["rev-parse", "feature^{commit}"]);

        let entry = EmbdEntry::load(&paths::embed_file_path(&f.folder())).unwrap();
        let outcome = process_entry(
            &f.root,
            Path::new("myembed"),
            entry,
            &args(Some("feature"), false),
        )
        .unwrap();
        match outcome {
            Outcome::Updated { new_commit, .. } => assert_eq!(new_commit, feature_tip),
            other => panic!("expected Updated, got {other:?}"),
        }
        assert_eq!(
            fs::read_to_string(f.folder().join("b.txt")).unwrap(),
            "beta"
        );
        let saved = EmbdEntry::load(&paths::embed_file_path(&f.folder())).unwrap();
        assert_eq!(saved.metadata.commit_hash, feature_tip);
        assert_eq!(
            saved.metadata.tracking,
            Some(Tracking::Branch("feature".into()))
        );
    }

    #[test]
    fn rev_at_current_pin_records_tracking() {
        // Migration path for pre-PR entries: a raw-hash pin that already
        // sits at the branch tip. The pin does not move, but the re-pin
        // must still record the tracking.
        let f = Fixture::embedded("main", None, &[("a.txt", "alpha")]);
        let tip = git_stdout(&f.repo, &["rev-parse", "main^{commit}"]);

        let entry = EmbdEntry::load(&paths::embed_file_path(&f.folder())).unwrap();
        let outcome = process_entry(
            &f.root,
            Path::new("myembed"),
            entry,
            &args(Some("main"), false),
        )
        .unwrap();
        match outcome {
            Outcome::Updated {
                new_commit,
                changes,
                ..
            } => {
                assert_eq!(new_commit, tip);
                assert!(
                    changes.is_empty(),
                    "expected no file churn, got {changes:?}"
                );
            }
            other => panic!("expected Updated, got {other:?}"),
        }
        let saved = EmbdEntry::load(&paths::embed_file_path(&f.folder())).unwrap();
        assert_eq!(saved.metadata.commit_hash, tip);
        assert_eq!(
            saved.metadata.tracking,
            Some(Tracking::Branch("main".into()))
        );
    }
}
