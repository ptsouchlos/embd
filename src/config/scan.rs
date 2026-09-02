use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::{
    color,
    config::{EmbdEntry, hash},
    filesystem,
};

/// Per-file finding produced when comparing the manifest against the folder.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FileChange {
    Modified(String),
    Deleted(String),
    Untracked(String),
    Symlink(String),
}

impl FileChange {
    /// Helper to convert [`FileChange`] to a marker (char).
    pub(crate) fn as_marker(&self) -> char {
        match self {
            Self::Modified(_) => 'M',
            Self::Deleted(_) => 'D',
            Self::Untracked(_) => '?',
            Self::Symlink(_) => 'L',
        }
    }
}

/// Aggregate report for a single entry. `Compared` and `FolderMissing` are
/// mutually exclusive.
#[derive(Debug)]
pub(crate) struct EntryReport {
    pub folder: PathBuf,
    pub state: EntryState,
    pub changes: Vec<FileChange>,
    pub allow_untracked: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EntryState {
    /// Folder exists. Drift status is determined by `changes`.
    Compared,
    // Folder missing
    FolderMissing,
}

impl EntryReport {
    pub(crate) fn has_drift(&self) -> bool {
        match self.state {
            EntryState::FolderMissing => true,
            EntryState::Compared => {
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

#[derive(Debug)]
pub(crate) enum DiskEntry {
    Regular { hash: String },
    Symlink,
}

pub(crate) fn inspect_entry(root: &Path, folder: &Path, entry: &EmbdEntry) -> EntryReport {
    let folder_abs = root.join(folder);
    let mut report = EntryReport {
        folder: folder.to_path_buf(),
        state: EntryState::Compared,
        changes: Vec::new(),
        allow_untracked: entry.metadata.allow_untracked,
    };

    if !folder_abs.exists() {
        report.state = EntryState::FolderMissing;
        return report;
    }

    let on_disk = match scan_folder(&folder_abs) {
        Ok(map) => map,
        Err(e) => {
            anstream::eprintln!(
                "{} failed to scan folder for '{}': {}",
                color::warning_label(),
                folder.display(),
                e
            );
            report.state = EntryState::FolderMissing;
            return report;
        }
    };

    // Modified / Deleted / Symlink-at-tracked-path
    for (key, expected_hash) in entry.files.as_map() {
        match on_disk.get(key) {
            Some(DiskEntry::Regular { hash }) if hash == expected_hash => {}
            Some(DiskEntry::Regular { .. }) => {
                report.changes.push(FileChange::Modified(key.clone()));
            }
            Some(DiskEntry::Symlink) => {
                report.changes.push(FileChange::Symlink(key.clone()));
            }
            None => {
                report.changes.push(FileChange::Deleted(key.clone()));
            }
        }
    }

    // Untracked / new-symlink
    for (key, disk_entry) in &on_disk {
        if entry.files.as_map().contains_key(key) {
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
    report
        .changes
        .sort_by_key(|c| (change_rank(c), key_of(c).to_string()));

    report
}

pub(crate) fn scan_folder(folder: &Path) -> Result<BTreeMap<String, DiskEntry>> {
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
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read file type of {}", entry.path().display()))?;
        let child_relative = relative.join(&name);
        let key = super::path_to_key(&child_relative);
        if file_type.is_symlink() {
            out.insert(key, DiskEntry::Symlink);
        } else if file_type.is_dir() {
            scan_folder_inner(root, &child_relative, out)?;
        } else if file_type.is_file() {
            let hash = hash::hash_file(&entry.path())?;
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

pub(crate) fn change_rank(c: &FileChange) -> u8 {
    match c {
        FileChange::Modified(_) => 0,
        FileChange::Deleted(_) => 1,
        FileChange::Untracked(_) => 2,
        FileChange::Symlink(_) => 3,
    }
}

pub(crate) fn key_of(c: &FileChange) -> &str {
    match c {
        FileChange::Modified(k)
        | FileChange::Deleted(k)
        | FileChange::Untracked(k)
        | FileChange::Symlink(k) => k,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FileLocks, Metadata};
    use std::fs;
    use tempfile::tempdir;

    /// Builds a folder under a fresh temp root with the given files, and an
    /// [`EmbdEntry`] whose `files` manifest is hashed from that folder — i.e.
    /// a "just synced" entry with no drift.
    fn fixture(commit: &str, files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf, EmbdEntry) {
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
        let locked_files = FileLocks::build_from_path(&folder).unwrap();
        let entry = EmbdEntry {
            metadata: Metadata {
                remote: "https://example.git".into(),
                commit_hash: commit.into(),
                allow_untracked: false,
                include: Vec::new(),
                exclude: Vec::new(),
            },
            files: locked_files,
        };
        (dir, root, entry)
    }

    #[test]
    fn reports_clean_when_folder_matches_manifest() {
        let (_dir, root, entry) = fixture("abc123", &[("a.txt", "alpha")]);
        let report = inspect_entry(&root, Path::new("vendor/foo"), &entry);
        assert_eq!(report.state, EntryState::Compared);
        assert!(report.changes.is_empty());
        assert!(!report.has_drift());
    }

    #[test]
    fn reports_modified_file() {
        let (_dir, root, entry) = fixture("abc123", &[("a.txt", "alpha")]);
        fs::write(root.join("vendor/foo/a.txt"), "ALPHA").unwrap();
        let report = inspect_entry(&root, Path::new("vendor/foo"), &entry);
        assert_eq!(report.changes, vec![FileChange::Modified("a.txt".into())]);
        assert!(report.has_drift());
    }

    #[test]
    fn reports_deleted_file() {
        let (_dir, root, entry) = fixture("abc123", &[("a.txt", "alpha"), ("b.txt", "beta")]);
        fs::remove_file(root.join("vendor/foo/a.txt")).unwrap();
        let report = inspect_entry(&root, Path::new("vendor/foo"), &entry);
        assert_eq!(report.changes, vec![FileChange::Deleted("a.txt".into())]);
        assert!(report.has_drift());
    }

    #[test]
    fn untracked_is_drift_when_flag_off() {
        let (_dir, root, entry) = fixture("abc123", &[("a.txt", "alpha")]);
        fs::write(root.join("vendor/foo/extra.txt"), "x").unwrap();
        let report = inspect_entry(&root, Path::new("vendor/foo"), &entry);
        assert_eq!(
            report.changes,
            vec![FileChange::Untracked("extra.txt".into())]
        );
        assert!(report.has_drift());
    }

    #[test]
    fn untracked_is_clean_when_flag_on() {
        let (_dir, root, mut entry) = fixture("abc123", &[("a.txt", "alpha")]);
        entry.metadata.allow_untracked = true;
        fs::write(root.join("vendor/foo/extra.txt"), "x").unwrap();
        let report = inspect_entry(&root, Path::new("vendor/foo"), &entry);
        assert_eq!(
            report.changes,
            vec![FileChange::Untracked("extra.txt".into())]
        );
        assert!(!report.has_drift());
    }

    #[test]
    fn modified_overrides_allow_untracked() {
        let (_dir, root, mut entry) = fixture("abc123", &[("a.txt", "alpha")]);
        entry.metadata.allow_untracked = true;
        fs::write(root.join("vendor/foo/a.txt"), "ALPHA").unwrap();
        fs::write(root.join("vendor/foo/extra.txt"), "x").unwrap();
        let report = inspect_entry(&root, Path::new("vendor/foo"), &entry);
        assert!(
            report.has_drift(),
            "modified file must always count as drift"
        );
    }

    #[test]
    fn crlf_checkout_of_lf_lockfile_is_not_drift() {
        // Simulates a manifest built from an LF checkout (e.g. on Linux/macOS)
        // being checked with a CRLF-converted working tree (e.g. Windows'
        // default core.autocrlf checkout behavior).
        let (_dir, root, entry) = fixture("abc123", &[("a.txt", "alpha\nbeta\n")]);
        fs::write(root.join("vendor/foo/a.txt"), "alpha\r\nbeta\r\n").unwrap();
        let report = inspect_entry(&root, Path::new("vendor/foo"), &entry);
        assert!(report.changes.is_empty(), "{:?}", report.changes);
        assert!(!report.has_drift());
    }

    #[test]
    fn folder_missing_reports_drift() {
        let (_dir, root, entry) = fixture("abc123", &[("a.txt", "alpha")]);
        fs::remove_dir_all(root.join("vendor/foo")).unwrap();
        let report = inspect_entry(&root, Path::new("vendor/foo"), &entry);
        assert_eq!(report.state, EntryState::FolderMissing);
        assert!(report.has_drift());
    }

    #[test]
    fn change_ordering_is_stable() {
        let (_dir, root, entry) = fixture("abc123", &[("a.txt", "alpha"), ("b.txt", "beta")]);
        fs::write(root.join("vendor/foo/b.txt"), "BETA").unwrap();
        fs::remove_file(root.join("vendor/foo/a.txt")).unwrap();
        fs::write(root.join("vendor/foo/z.txt"), "z").unwrap();
        let report = inspect_entry(&root, Path::new("vendor/foo"), &entry);
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
        let (_dir, root, mut entry) = fixture("abc123", &[("a.txt", "alpha")]);
        entry.metadata.allow_untracked = true;
        symlink("a.txt", root.join("vendor/foo/link.txt")).unwrap();
        let report = inspect_entry(&root, Path::new("vendor/foo"), &entry);
        assert_eq!(report.changes, vec![FileChange::Symlink("link.txt".into())]);
        assert!(report.has_drift());
    }

    #[test]
    fn scan_folder_skips_embd_marker() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".embd"), "marker").unwrap();
        fs::write(dir.path().join("keep.txt"), "k").unwrap();
        let scanned = scan_folder(dir.path()).unwrap();
        assert!(scanned.contains_key("keep.txt"));
        assert!(!scanned.contains_key(".embd"));
    }
}
