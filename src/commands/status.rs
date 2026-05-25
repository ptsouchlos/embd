use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::cache::{self, Manifest};
use crate::config::{self, Config, EmbdEntry};
use crate::filesystem;
use crate::paths;

#[derive(clap::Args, Debug)]
pub(crate) struct StatusArgs {
    /// Optional names to check. When empty, all embeds in the config are checked.
    names: Vec<String>,
    /// Suppress per-file rows; print only one-line summaries per entry.
    #[clap(short, long)]
    quiet: bool,
}

pub(crate) fn execute(args: StatusArgs) -> Result<()> {
    let root = paths::find_git_root()?;
    let config_path = paths::config_path(&root);
    let config = config::load_or_default(&config_path)?;
    let selected = select_entries(&config, &args.names)?;

    let mut any_drift = false;
    for (name, entry) in selected {
        let report = inspect_entry(&root, name, entry);
        any_drift |= report.has_drift();
        print_report(&report, args.quiet);
    }

    if any_drift {
        std::process::exit(1);
    }
    Ok(())
}

fn select_entries<'a>(
    config: &'a Config,
    names: &'a [String],
) -> Result<Vec<(&'a str, &'a EmbdEntry)>> {
    if names.is_empty() {
        return Ok(config.iter().collect());
    }
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let entry = config
            .get(name)
            .with_context(|| format!("no embed named '{name}' in config"))?;
        out.push((name.as_str(), entry));
    }
    Ok(out)
}

/// Per-file finding produced when comparing the manifest against the folder.
#[derive(Debug, PartialEq, Eq)]
enum FileChange {
    Modified(String),
    Deleted(String),
    Untracked(String),
    Symlink(String),
}

/// Aggregate report for a single entry. `Clean`, `FolderMissing`, and
/// `NoCache` are mutually exclusive; `stale` and `changes` can coexist.
#[derive(Debug)]
struct EntryReport<'a> {
    name: &'a str,
    folder: PathBuf,
    state: EntryState,
    stale: Option<(String, String)>, // (folder commit, config commit)
    changes: Vec<FileChange>,
    allow_untracked: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum EntryState {
    /// Folder + manifest exist and match the config commit. Drift status is
    /// determined by `changes` and (in error-mode) untracked files.
    Compared,
    FolderMissing,
    NoCache,
}

impl EntryReport<'_> {
    fn has_drift(&self) -> bool {
        match self.state {
            EntryState::FolderMissing => true,
            EntryState::NoCache => false,
            EntryState::Compared => {
                if self.stale.is_some() {
                    return true;
                }
                for change in &self.changes {
                    match change {
                        FileChange::Untracked(_) if self.allow_untracked => {}
                        _ => return true,
                    }
                }
                false
            }
        }
    }
}

fn inspect_entry<'a>(root: &Path, name: &'a str, entry: &EmbdEntry) -> EntryReport<'a> {
    let folder_abs = root.join(&entry.folder);
    let mut report = EntryReport {
        name,
        folder: entry.folder.clone(),
        state: EntryState::Compared,
        stale: None,
        changes: Vec::new(),
        allow_untracked: entry.allow_untracked,
    };

    if !folder_abs.exists() {
        report.state = EntryState::FolderMissing;
        return report;
    }

    let manifest_path = paths::cache_path(root, name);
    let manifest = match Manifest::load(&manifest_path) {
        Ok(m) => m,
        Err(_) if !manifest_path.exists() => {
            report.state = EntryState::NoCache;
            return report;
        }
        Err(e) => {
            // The manifest is unreadable but present — treat as no-cache and
            // surface the error in the rendered line.
            eprintln!(
                "warning: failed to load manifest for '{}': {}",
                name, e
            );
            report.state = EntryState::NoCache;
            return report;
        }
    };

    if manifest.commit_hash != entry.commit_hash {
        report.stale = Some((manifest.commit_hash.clone(), entry.commit_hash.clone()));
    }

    let on_disk = match scan_folder(&folder_abs) {
        Ok(map) => map,
        Err(e) => {
            eprintln!(
                "warning: failed to scan folder for '{}': {}",
                name, e
            );
            report.state = EntryState::FolderMissing;
            return report;
        }
    };

    // Modified / Deleted
    for (key, expected_hash) in &manifest.files {
        match on_disk.get(key) {
            Some(DiskEntry::Regular { hash }) if hash == expected_hash => {}
            Some(DiskEntry::Regular { .. }) => {
                report.changes.push(FileChange::Modified(key.clone()));
            }
            Some(DiskEntry::Symlink) => {
                // A previously-tracked file is now a symlink. Treat as drift.
                report.changes.push(FileChange::Symlink(key.clone()));
            }
            None => {
                report.changes.push(FileChange::Deleted(key.clone()));
            }
        }
    }

    // Untracked / new-symlink
    for (key, disk_entry) in &on_disk {
        if manifest.files.contains_key(key) {
            continue;
        }
        match disk_entry {
            DiskEntry::Regular { .. } => {
                report.changes.push(FileChange::Untracked(key.clone()));
            }
            DiskEntry::Symlink => {
                report.changes.push(FileChange::Symlink(key.clone()));
            }
        }
    }

    // Stable output order: Modified, Deleted, Untracked, Symlink — within each
    // group, lexicographic.
    report.changes.sort_by_key(|c| (change_rank(c), key_of(c).to_string()));

    report
}

#[derive(Debug)]
enum DiskEntry {
    Regular { hash: String },
    Symlink,
}

fn scan_folder(folder: &Path) -> Result<BTreeMap<String, DiskEntry>> {
    let mut out = BTreeMap::new();
    scan_folder_inner(folder, Path::new(""), &mut out)?;
    Ok(out)
}

fn scan_folder_inner(
    root: &Path,
    relative: &Path,
    out: &mut BTreeMap<String, DiskEntry>,
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
        let file_type = entry.file_type().with_context(|| {
            format!("failed to read file type of {}", entry.path().display())
        })?;
        let child_relative = relative.join(&name);
        let key = path_to_key(&child_relative);
        if file_type.is_symlink() {
            out.insert(key, DiskEntry::Symlink);
        } else if file_type.is_dir() {
            scan_folder_inner(root, &child_relative, out)?;
        } else if file_type.is_file() {
            let hash = cache::hash_file(&entry.path())?;
            out.insert(key, DiskEntry::Regular { hash });
        } else {
            bail!(
                "encountered unsupported file type at {}",
                entry.path().display()
            );
        }
    }
    Ok(())
}

fn path_to_key(relative: &Path) -> String {
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn change_rank(c: &FileChange) -> u8 {
    match c {
        FileChange::Modified(_) => 0,
        FileChange::Deleted(_) => 1,
        FileChange::Untracked(_) => 2,
        FileChange::Symlink(_) => 3,
    }
}

fn key_of(c: &FileChange) -> &str {
    match c {
        FileChange::Modified(k)
        | FileChange::Deleted(k)
        | FileChange::Untracked(k)
        | FileChange::Symlink(k) => k,
    }
}

fn print_report(report: &EntryReport, quiet: bool) {
    let header = format!("{} ({})", report.name, report.folder.display());

    match report.state {
        EntryState::FolderMissing => {
            println!("{header}: folder missing");
            return;
        }
        EntryState::NoCache => {
            println!("{header}: no cache");
            return;
        }
        EntryState::Compared => {}
    }

    let summary = if let Some((local, wanted)) = &report.stale {
        format!("stale (folder at {local}, config wants {wanted})")
    } else if report.changes.is_empty() {
        "clean".to_string()
    } else {
        "drift".to_string()
    };
    println!("{header}: {summary}");

    if quiet {
        return;
    }
    for change in &report.changes {
        match change {
            FileChange::Modified(p) => println!("  M  {p}"),
            FileChange::Deleted(p) => println!("  D  {p}"),
            FileChange::Untracked(p) => {
                if report.allow_untracked {
                    println!("  ?  {p} (untracked, allowed)");
                } else {
                    println!("  ?  {p} (untracked)");
                }
            }
            FileChange::Symlink(p) => println!("  L  {p} (symlink)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn make_entry(folder: &str, commit: &str, allow_untracked: bool) -> EmbdEntry {
        EmbdEntry {
            remote: "https://example.git".into(),
            commit_hash: commit.into(),
            folder: PathBuf::from(folder),
            allow_untracked,
        }
    }

    /// Build a temp git-root-shaped fixture: write some files, build a manifest,
    /// save it under the proper cache path.
    fn fixture(commit: &str, files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf, String) {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let folder = root.join("vendor/foo");
        fs::create_dir_all(&folder).unwrap();
        for (rel, contents) in files {
            let path = folder.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, contents).unwrap();
        }
        let manifest = Manifest::build_from_path(&folder, commit.into()).unwrap();
        let manifest_path = paths::cache_path(&root, "foo");
        manifest.save(&manifest_path).unwrap();
        (dir, root, "foo".into())
    }

    #[test]
    fn reports_clean_when_folder_matches_manifest() {
        let (_dir, root, name) = fixture("abc123", &[("a.txt", "alpha")]);
        let entry = make_entry("vendor/foo", "abc123", false);
        let report = inspect_entry(&root, &name, &entry);
        assert_eq!(report.state, EntryState::Compared);
        assert!(report.changes.is_empty());
        assert!(report.stale.is_none());
        assert!(!report.has_drift());
    }

    #[test]
    fn reports_modified_file() {
        let (_dir, root, name) = fixture("abc123", &[("a.txt", "alpha")]);
        let entry = make_entry("vendor/foo", "abc123", false);
        fs::write(root.join("vendor/foo/a.txt"), "ALPHA").unwrap();
        let report = inspect_entry(&root, &name, &entry);
        assert_eq!(report.changes, vec![FileChange::Modified("a.txt".into())]);
        assert!(report.has_drift());
    }

    #[test]
    fn reports_deleted_file() {
        let (_dir, root, name) = fixture("abc123", &[("a.txt", "alpha"), ("b.txt", "beta")]);
        let entry = make_entry("vendor/foo", "abc123", false);
        fs::remove_file(root.join("vendor/foo/a.txt")).unwrap();
        let report = inspect_entry(&root, &name, &entry);
        assert_eq!(report.changes, vec![FileChange::Deleted("a.txt".into())]);
        assert!(report.has_drift());
    }

    #[test]
    fn untracked_is_drift_when_flag_off() {
        let (_dir, root, name) = fixture("abc123", &[("a.txt", "alpha")]);
        let entry = make_entry("vendor/foo", "abc123", false);
        fs::write(root.join("vendor/foo/extra.txt"), "x").unwrap();
        let report = inspect_entry(&root, &name, &entry);
        assert_eq!(report.changes, vec![FileChange::Untracked("extra.txt".into())]);
        assert!(report.has_drift());
    }

    #[test]
    fn untracked_is_clean_when_flag_on() {
        let (_dir, root, name) = fixture("abc123", &[("a.txt", "alpha")]);
        let entry = make_entry("vendor/foo", "abc123", true);
        fs::write(root.join("vendor/foo/extra.txt"), "x").unwrap();
        let report = inspect_entry(&root, &name, &entry);
        assert_eq!(report.changes, vec![FileChange::Untracked("extra.txt".into())]);
        // Allowed → not drift even though there are untracked files.
        assert!(!report.has_drift());
    }

    #[test]
    fn modified_overrides_allow_untracked() {
        let (_dir, root, name) = fixture("abc123", &[("a.txt", "alpha")]);
        let entry = make_entry("vendor/foo", "abc123", true);
        fs::write(root.join("vendor/foo/a.txt"), "ALPHA").unwrap();
        fs::write(root.join("vendor/foo/extra.txt"), "x").unwrap();
        let report = inspect_entry(&root, &name, &entry);
        assert!(report.has_drift(), "modified file must always count as drift");
    }

    #[test]
    fn stale_when_config_commit_differs() {
        let (_dir, root, name) = fixture("abc123", &[("a.txt", "alpha")]);
        let entry = make_entry("vendor/foo", "def456", false);
        let report = inspect_entry(&root, &name, &entry);
        assert_eq!(
            report.stale,
            Some(("abc123".into(), "def456".into()))
        );
        assert!(report.has_drift());
    }

    #[test]
    fn folder_missing_reports_drift() {
        let (_dir, root, name) = fixture("abc123", &[("a.txt", "alpha")]);
        let entry = make_entry("vendor/foo", "abc123", false);
        fs::remove_dir_all(root.join("vendor/foo")).unwrap();
        let report = inspect_entry(&root, &name, &entry);
        assert_eq!(report.state, EntryState::FolderMissing);
        assert!(report.has_drift());
    }

    #[test]
    fn no_cache_when_manifest_missing() {
        let (_dir, root, name) = fixture("abc123", &[("a.txt", "alpha")]);
        let entry = make_entry("vendor/foo", "abc123", false);
        fs::remove_file(paths::cache_path(&root, &name)).unwrap();
        let report = inspect_entry(&root, &name, &entry);
        assert_eq!(report.state, EntryState::NoCache);
        // no-cache is informational, not drift.
        assert!(!report.has_drift());
    }

    #[test]
    fn change_ordering_is_stable() {
        let (_dir, root, name) =
            fixture("abc123", &[("a.txt", "alpha"), ("b.txt", "beta")]);
        let entry = make_entry("vendor/foo", "abc123", false);
        fs::write(root.join("vendor/foo/b.txt"), "BETA").unwrap();
        fs::remove_file(root.join("vendor/foo/a.txt")).unwrap();
        fs::write(root.join("vendor/foo/z.txt"), "z").unwrap();
        let report = inspect_entry(&root, &name, &entry);
        assert_eq!(
            report.changes,
            vec![
                FileChange::Modified("b.txt".into()),
                FileChange::Deleted("a.txt".into()),
                FileChange::Untracked("z.txt".into()),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_reported_as_drift() {
        use std::os::unix::fs::symlink;
        let (_dir, root, name) = fixture("abc123", &[("a.txt", "alpha")]);
        let entry = make_entry("vendor/foo", "abc123", true);
        symlink("a.txt", root.join("vendor/foo/link.txt")).unwrap();
        let report = inspect_entry(&root, &name, &entry);
        assert_eq!(report.changes, vec![FileChange::Symlink("link.txt".into())]);
        // Symlinks count as drift even with allow_untracked.
        assert!(report.has_drift());
    }
}
