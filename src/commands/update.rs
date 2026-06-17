use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tempfile::tempdir;

use crate::commands::common::select_entries;
use crate::commands::status::print_report;
use crate::config::{self, Config, EmbdEntry};
use crate::filter::Filter;
use crate::lockfile::{self, EntryState, FileChange, LockEntry, Lockfile};
use crate::{git, paths};

#[derive(clap::Args, Debug)]
pub(crate) struct UpdateArgs {
    /// Optional names to update. When empty, every entry in the config is updated.
    names: Vec<String>,
    /// Rev (commit hash, tag, or branch) to advance the entry's pin to.
    /// Requires exactly one name.
    #[clap(short, long)]
    rev: Option<String>,
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
    let config_path = paths::config_path(&root);
    let mut config = config::load_or_default(&config_path)?;
    let lock_path = paths::lock_path(&root);
    let mut lock = Lockfile::load_or_default(&lock_path)?;

    let selected: Vec<(String, EmbdEntry)> = select_entries(&config, &args.names)?
        .into_iter()
        .map(|(n, e)| (n.to_string(), e.clone()))
        .collect();

    if args.rev.is_some() && args.names.len() != 1 {
        bail!("--rev requires exactly one name to be specified");
    }

    let mut any_failed = false;
    for (name, entry) in &selected {
        match process_entry(
            &root,
            name,
            entry,
            &args,
            &mut config,
            &config_path,
            &mut lock,
            &lock_path,
        ) {
            Ok(outcome) => {
                print_outcome(name, entry, &outcome, args.quiet);
                if outcome.is_failure() {
                    any_failed = true;
                }
            }
            Err(e) => {
                eprintln!("error: {name}: {e:#}");
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
    /// Clean, not stale, no `--rev` (or rev resolves to the existing pin). No clone done.
    UpToDate,
    /// Drift detected, `--force` not given. Drift was already printed via `print_report`.
    SkippedDrift,
    /// Lockfile missing, `--force` not given.
    SkippedNoLockfile,
    /// Applied: files synced, manifest saved, config saved if `--rev` bumped the pin.
    Updated {
        old_commit: String,
        new_commit: String,
        changes: Vec<UpdateChange>,
    },
}

impl Outcome {
    fn is_failure(&self) -> bool {
        matches!(self, Outcome::SkippedDrift | Outcome::SkippedNoLockfile)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum UpdateChange {
    Wrote(String),
    Deleted(String),
    Removed(String), // --overwrite swept this
}

#[allow(clippy::too_many_arguments)]
fn process_entry(
    root: &Path,
    name: &str,
    entry: &EmbdEntry,
    args: &UpdateArgs,
    config: &mut Config,
    config_path: &Path,
    lock: &mut Lockfile,
    lock_path: &Path,
) -> Result<Outcome> {
    let report = lockfile::inspect_entry(root, name, entry, lock.get(name));

    // Short-circuit no-op: only safe when no --rev was requested. With --rev we
    // must still resolve the ref to know whether it changes the pin.
    if args.rev.is_none()
        && report.state == EntryState::Compared
        && report.stale.is_none()
        && report.changes.is_empty()
    {
        return Ok(Outcome::UpToDate);
    }

    // No lockfile gate. Folder exists but we have no manifest to diff against.
    if report.state == EntryState::Missing && !args.force {
        return Ok(Outcome::SkippedNoLockfile);
    }

    // Drift gate. Only file-level drift requires --force; pure staleness is what
    // update exists to apply.
    let has_file_drift = report.state == EntryState::Compared
        && report.changes.iter().any(|c| match c {
            FileChange::Untracked(_) => !entry.allow_untracked,
            _ => true,
        });
    if has_file_drift && !args.force {
        print_report(&report, args.quiet);
        return Ok(Outcome::SkippedDrift);
    }

    // Clone the target rev to a tempdir and resolve to a concrete commit.
    // Always full clone: the rev we check out may be any historical commit
    // (entry.commit_hash for re-sync, or args.rev for a bump), not HEAD.
    let tmp = tempdir().context("failed to create temporary directory")?;
    git::cli::clone(&entry.remote, tmp.path(), false)?;
    let rev_to_checkout = args.rev.as_ref().unwrap_or(&entry.commit_hash);
    git::cli::checkout(tmp.path(), rev_to_checkout.clone())?;
    let new_commit = git::cli::commit_hash_of(tmp.path())?;

    // If --rev resolved to the existing pin and there's no drift, this is also a no-op.
    if args.rev.is_some()
        && new_commit == entry.commit_hash
        && report.state == EntryState::Compared
        && report.stale.is_none()
        && report.changes.is_empty()
    {
        return Ok(Outcome::UpToDate);
    }

    // Folder-missing recovery: create the destination folder. Lossless.
    let folder_abs = root.join(&entry.folder);
    if report.state == EntryState::FolderMissing {
        std::fs::create_dir_all(&folder_abs)
            .with_context(|| format!("failed to create folder {}", folder_abs.display()))?;
    }

    let old_commit = entry.commit_hash.clone();

    // Save config first when --rev moves the pin. Durable pin is the source of
    // truth; a crash after this point is recoverable via `update --force`.
    if args.rev.is_some() && new_commit != entry.commit_hash {
        let e = config
            .get_mut(name)
            .with_context(|| format!("entry '{name}' disappeared from config"))?;
        e.commit_hash = new_commit.clone();
        config.save(config_path)?;
    }

    // Build the prospective lock entry from the temp clone, applying the same
    // include/exclude filter the entry was added with so filtered-out files are
    // never re-introduced.
    let filter = Filter::from_patterns(&entry.include, &entry.exclude)?;
    let new_entry = LockEntry::build_from_path_filtered(tmp.path(), new_commit.clone(), &filter)?;

    // Take the old file list from the lock entry, or treat as empty for
    // no-lockfile / folder-missing.
    let old_files: BTreeMap<String, String> = match report.state {
        EntryState::Compared => lock.get(name).map(|e| e.files.clone()).unwrap_or_default(),
        EntryState::Missing | EntryState::FolderMissing => BTreeMap::new(),
    };

    let changes = apply_update(
        tmp.path(),
        &folder_abs,
        &old_files,
        &new_entry.files,
        args.overwrite,
    )?;

    // Save the updated lock file last.
    lock.upsert(name.to_string(), new_entry);
    lock.save(lock_path)?;

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
            && let Ok(disk_hash) = lockfile::hash_file(&dst_path)
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
        let on_disk = lockfile::scan_folder(dst).unwrap_or_default();
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

fn print_outcome(name: &str, entry: &EmbdEntry, outcome: &Outcome, quiet: bool) {
    let header = format!("{name} ({})", entry.folder.display());
    match outcome {
        Outcome::UpToDate => println!("{header}: up to date"),
        Outcome::SkippedNoLockfile => println!("{header}: no lock file (use --force)"),
        Outcome::SkippedDrift => {
            // The detailed diff was already printed via print_report. Add a trailing
            // hint so the user knows what to do.
            eprintln!("error: re-run with --force to overwrite local modifications");
        }
        Outcome::Updated {
            old_commit,
            new_commit,
            changes,
        } => {
            let n = changes.len();
            let plural = if n == 1 { "change" } else { "changes" };
            if old_commit == new_commit {
                println!("{header}: updated, {n} {plural}");
            } else {
                println!(
                    "{header}: updated {} -> {}, {n} {plural}",
                    short(old_commit),
                    short(new_commit)
                );
            }
            if !quiet {
                for c in changes {
                    match c {
                        UpdateChange::Wrote(p) => println!("  W  {p}"),
                        UpdateChange::Deleted(p) => println!("  D  {p}"),
                        UpdateChange::Removed(p) => {
                            println!("  X  {p} (untracked, removed)")
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
    use std::fs;
    use tempfile::tempdir;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn manifest_for(folder: &Path, commit: &str) -> BTreeMap<String, String> {
        LockEntry::build_from_path(folder, commit.to_string())
            .unwrap()
            .files
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
}
