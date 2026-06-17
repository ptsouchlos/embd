use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tempfile::tempdir;

use crate::config::{self, EmbdEntry};
use crate::filter::Filter;
use crate::lockfile::{self, LockEntry};
use crate::{filesystem, git, paths};

/// Input arguments for the `add` command.
#[derive(clap::Args, Debug)]
pub(crate) struct AddArgs {
    #[clap(short, long, help = "Link to the repository")]
    link: String,
    #[clap(short, long, help = "Path to pull the files to")]
    folder: PathBuf,
    #[clap(
        short,
        long,
        help = "Turn on to allow untracked files in target folder"
    )]
    allow_untracked: bool,
    #[clap(short, long, help = "Commit hash, tag, or branch to pull")]
    rev: Option<String>,
    #[clap(
        short,
        long,
        help = "Comma-separated glob patterns; only matching files are pulled"
    )]
    include: Option<String>,
    #[clap(
        short,
        long,
        help = "Comma-separated glob patterns; matching files are skipped (wins over --include)"
    )]
    exclude: Option<String>,
}

/// Split a comma-separated pattern argument into a list of trimmed, non-empty
/// patterns. `None` and the empty string both yield an empty list. This is used to
/// parse include/exclude patterns from the command line input from the user.
fn parse_patterns(arg: &Option<String>) -> Vec<String> {
    arg.as_deref()
        .into_iter()
        .flat_map(|s| s.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub(crate) fn execute(args: AddArgs) -> Result<()> {
    let root = paths::find_git_root()?;
    let config_path = paths::config_path(&root);
    let cwd = std::env::current_dir().context("failed to read current directory")?;

    // Validate the link and filter patterns before doing any I/O.
    let (link, repo_name) = git::parse_repo_link(&args.link)?;
    let (folder_abs, folder_rel) = paths::resolve_inside_root(&args.folder, &root, &cwd)?;
    let include = parse_patterns(&args.include);
    let exclude = parse_patterns(&args.exclude);
    let filter = Filter::from_patterns(&include, &exclude)?;

    // Load the config, or use a default one
    let mut config = config::load_or_default(&config_path)?;

    // Check if the config already contains an entry for the given repo.
    if config.contains(&repo_name) {
        bail!(
            "an embed named '{}' already exists in {}",
            repo_name,
            config_path.display()
        );
    }

    // Does the folder the user wants to populate already exist?
    let folder_existed = folder_abs.exists();
    if folder_existed && !args.allow_untracked {
        let mut entries = std::fs::read_dir(&folder_abs)
            .with_context(|| format!("failed to read directory {}", folder_abs.display()))?;
        if entries.next().is_some() {
            bail!(
                "folder '{}' is non-empty; use --allow-untracked to proceed anyway",
                args.folder.display()
            );
        }
    }

    let tmp_dir = tempdir().context("failed to create temporary directory")?;
    let shallow = args.rev.is_none();
    git::cli::clone(&link, tmp_dir.path(), shallow)?;
    if let Some(ref rev) = args.rev {
        git::cli::checkout(tmp_dir.path(), rev.clone())?;
    }

    let commit_hash = git::cli::commit_hash_of(tmp_dir.path())?;

    // Copy + register. Anything that fails after copy_dir triggers a rollback
    // so we never leave on-disk state without a matching config entry.
    filesystem::copy_dir(tmp_dir.path(), &folder_abs, &filter)?;

    let lock_path = paths::lock_path(&root);

    let result = (|| -> Result<()> {
        let lock_entry = LockEntry::build_from_path(&folder_abs, commit_hash.clone())?;
        let mut lock = lockfile::Lockfile::load_or_default(&lock_path)?;
        lock.upsert(repo_name.clone(), lock_entry);
        lock.save(&lock_path)?;
        config.insert(
            repo_name.clone(),
            EmbdEntry {
                remote: link,
                commit_hash,
                folder: folder_rel,
                allow_untracked: args.allow_untracked,
                include,
                exclude,
            },
        )?;
        config.save(&config_path)?;
        Ok(())
    })();

    if let Err(e) = result {
        // If there was an error, try to rollback to the previous state.
        rollback(&folder_abs, folder_existed, args.allow_untracked);
        // Drop the entry we may have written to the lock file, leaving any other
        // entries untouched.
        if let Ok(mut lock) = lockfile::Lockfile::load(&lock_path)
            && lock.remove(&repo_name).is_some()
        {
            let _ = lock.save(&lock_path);
        }
        // Propagate the error up to the caller
        return Err(e);
    }

    Ok(())
}

/// Best-effort cleanup of a partially-installed embed. If the destination
/// folder didn't exist before, remove it entirely. If it existed and the user
/// passed --allow-untracked we don't touch it (we can't tell our files apart
/// from theirs); otherwise it was empty so we can safely remove it.
fn rollback(folder: &Path, folder_existed: bool, allow_untracked: bool) {
    if allow_untracked && folder_existed {
        eprintln!(
            "warning: leaving partially-copied files in '{}' (used --allow-untracked)",
            folder.display()
        );
        return;
    }
    if let Err(e) = std::fs::remove_dir_all(folder) {
        eprintln!(
            "warning: failed to clean up '{}' after error: {}",
            folder.display(),
            e
        );
    }
}
