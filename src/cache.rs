//! Per-entry hash manifest used by `embd status` to detect drift between the
//! files on disk and what was originally synced by `add`. The manifest is
//! intentionally gitignored — embed maintenance is the maintainer's burden,
//! not the consumer's.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::config::EmbdEntry;
use crate::filesystem;
use crate::paths;

const CURRENT_SCHEMA: u32 = 1;
const HASH_BUFFER_SIZE: usize = 64 * 1024;

/// Snapshot of a synced embed's contents. The `commit_hash` field records the
/// commit the folder reflects, distinct from [`crate::config::EmbdEntry::commit_hash`]
/// which records the commit the user *wants*. A mismatch between them is the
/// "stale" status.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub schema_version: u32,
    pub commit_hash: String,
    pub files: BTreeMap<String, String>,
}

impl Manifest {
    /// Build a manifest by hashing every regular file under `root` (excluding
    /// `.git`, never following symlinks).
    pub(crate) fn build_from_path(root: &Path, commit_hash: String) -> Result<Self> {
        let mut files = BTreeMap::new();
        for relative in walk_files(root)? {
            let absolute = root.join(&relative);
            let hash = hash_file(&absolute)?;
            files.insert(path_to_key(&relative), hash);
        }
        Ok(Self {
            schema_version: CURRENT_SCHEMA,
            commit_hash,
            files,
        })
    }

    pub(crate) fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read manifest from {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("failed to parse manifest at {}", path.display()))
    }

    /// Save the manifest with a tmpfile-then-rename so a crash mid-write can't
    /// leave a torn file.
    pub(crate) fn save(&self, path: &Path) -> Result<()> {
        let parent = path.parent().with_context(|| {
            format!("manifest path {} has no parent directory", path.display())
        })?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
        let content = toml::to_string_pretty(self).context("failed to serialize manifest")?;
        let mut tmp = NamedTempFile::new_in(parent)
            .with_context(|| format!("failed to create temp file in {}", parent.display()))?;
        tmp.write_all(content.as_bytes())
            .context("failed to write manifest content")?;
        tmp.persist(path)
            .with_context(|| format!("failed to persist manifest to {}", path.display()))?;
        Ok(())
    }
}

/// Walk a directory tree and return every regular file's path relative to
/// `root`, sorted lexicographically. Skips `.git` directories and silently
/// ignores symlinks (the status walker reports symlinks separately).
fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk_files_inner(root, Path::new(""), &mut out)?;
    out.sort();
    Ok(out)
}

fn walk_files_inner(root: &Path, relative: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
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
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            walk_files_inner(root, &child_relative, out)?;
        } else if file_type.is_file() {
            out.push(child_relative);
        }
    }
    Ok(())
}

/// Hash a file's contents with SHA-256, streamed in 64 KiB chunks so multi-GiB
/// vendored blobs don't blow up memory.
pub(crate) fn hash_file(path: &Path) -> Result<String> {
    let file = File::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; HASH_BUFFER_SIZE];
    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(format!("sha256:{:x}", base16ct::HexDisplay(&digest)))
}

/// Convert a relative path to the string key used in the manifest. Paths are
/// stored with forward slashes regardless of host OS so manifests stay portable.
pub(crate) fn path_to_key(relative: &Path) -> String {
    let mut parts = Vec::new();
    for component in relative.components() {
        parts.push(component.as_os_str().to_string_lossy().into_owned());
    }
    parts.join("/")
}

/// Per-file finding produced when comparing the manifest against the folder.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FileChange {
    Modified(String),
    Deleted(String),
    Untracked(String),
    Symlink(String),
}

/// Aggregate report for a single entry. `Compared`, `FolderMissing`, and
/// `NoCache` are mutually exclusive; `stale` and `changes` can coexist within
/// `Compared`.
#[derive(Debug)]
pub(crate) struct EntryReport<'a> {
    pub name: &'a str,
    pub folder: PathBuf,
    pub state: EntryState,
    pub stale: Option<(String, String)>, // (folder commit, config commit)
    pub changes: Vec<FileChange>,
    pub allow_untracked: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EntryState {
    /// Folder + manifest exist. Drift status is determined by `stale` and `changes`.
    Compared,
    FolderMissing,
    NoCache,
}

impl EntryReport<'_> {
    pub(crate) fn has_drift(&self) -> bool {
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

#[derive(Debug)]
pub(crate) enum DiskEntry {
    Regular { hash: String },
    Symlink,
}

pub(crate) fn inspect_entry<'a>(
    root: &Path,
    name: &'a str,
    entry: &EmbdEntry,
) -> EntryReport<'a> {
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
            // Manifest present but unreadable — surface and treat as no-cache.
            eprintln!("warning: failed to load manifest for '{}': {}", name, e);
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
            eprintln!("warning: failed to scan folder for '{}': {}", name, e);
            report.state = EntryState::FolderMissing;
            return report;
        }
    };

    // Modified / Deleted / Symlink-at-tracked-path
    for (key, expected_hash) in &manifest.files {
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
        let key = path_to_key(&child_relative);
        if file_type.is_symlink() {
            out.insert(key, DiskEntry::Symlink);
        } else if file_type.is_dir() {
            scan_folder_inner(root, &child_relative, out)?;
        } else if file_type.is_file() {
            let hash = hash_file(&entry.path())?;
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
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn make_entry(folder: &str, commit: &str, allow_untracked: bool) -> EmbdEntry {
        EmbdEntry {
            remote: "https://example.git".into(),
            commit_hash: commit.into(),
            folder: PathBuf::from(folder),
            allow_untracked,
        }
    }

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
        assert_eq!(report.stale, Some(("abc123".into(), "def456".into())));
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
        assert!(!report.has_drift());
    }

    #[test]
    fn change_ordering_is_stable() {
        let (_dir, root, name) = fixture("abc123", &[("a.txt", "alpha"), ("b.txt", "beta")]);
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
        assert!(report.has_drift());
    }

    #[test]
    fn hash_file_matches_known_sha256() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("data");
        std::fs::write(&path, b"hello world").unwrap();
        let hash = hash_file(&path).unwrap();
        // sha256("hello world") = b94d27b9...
        assert_eq!(
            hash,
            "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn hash_file_streams_large_inputs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("big");
        // 1 MiB of zeros — exercises the buffer loop without straining memory.
        let data = vec![0u8; 1024 * 1024];
        std::fs::write(&path, &data).unwrap();
        let hash = hash_file(&path).unwrap();
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash.len(), "sha256:".len() + 64);
    }

    #[test]
    fn walk_files_returns_sorted_relative_paths() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("b/sub")).unwrap();
        std::fs::write(dir.path().join("b/sub/c.txt"), "c").unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("b/b.txt"), "b").unwrap();

        let files = walk_files(dir.path()).unwrap();
        assert_eq!(
            files,
            vec![
                PathBuf::from("a.txt"),
                PathBuf::from("b").join("b.txt"),
                PathBuf::from("b").join("sub").join("c.txt"),
            ]
        );
    }

    #[test]
    fn walk_files_skips_git_directory() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "x").unwrap();
        std::fs::write(dir.path().join("keep.txt"), "k").unwrap();
        let files = walk_files(dir.path()).unwrap();
        assert_eq!(files, vec![PathBuf::from("keep.txt")]);
    }

    #[cfg(unix)]
    #[test]
    fn walk_files_skips_symlinks() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("real.txt"), "r").unwrap();
        symlink("real.txt", dir.path().join("link.txt")).unwrap();
        let files = walk_files(dir.path()).unwrap();
        assert_eq!(files, vec![PathBuf::from("real.txt")]);
    }

    #[test]
    fn manifest_round_trip() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha").unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.txt"), "beta").unwrap();

        let manifest = Manifest::build_from_path(dir.path(), "abc123".into()).unwrap();
        assert_eq!(manifest.commit_hash, "abc123");
        assert_eq!(manifest.files.len(), 2);
        assert!(manifest.files.contains_key("a.txt"));
        assert!(manifest.files.contains_key("sub/b.txt"));

        let path = dir.path().join("manifest.toml");
        manifest.save(&path).unwrap();
        let loaded = Manifest::load(&path).unwrap();
        assert_eq!(loaded.commit_hash, "abc123");
        assert_eq!(loaded.schema_version, CURRENT_SCHEMA);
        assert_eq!(loaded.files, manifest.files);
    }

    #[test]
    fn manifest_save_creates_parent_directory() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested/cache/foo.toml");
        let manifest = Manifest {
            schema_version: CURRENT_SCHEMA,
            commit_hash: "deadbeef".into(),
            files: BTreeMap::new(),
        };
        manifest.save(&path).unwrap();
        assert!(path.exists());
    }
}
