use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tempfile::tempdir;

use crate::cache::Manifest;
use crate::config::{self, EmbdEntry};
use crate::{filesystem, git, paths};

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
    #[clap(short, long, help = "Commit hash or tag to pull")]
    git_tag: Option<String>,
}

pub(crate) fn execute(args: AddArgs) -> Result<()> {
    let root = paths::find_git_root()?;
    let config_path = paths::config_path(&root);
    let cwd = std::env::current_dir().context("failed to read current directory")?;

    // Validate the link before doing any I/O.
    let (link, repo_name) = git::parse_repo_link(&args.link)?;
    let (folder_abs, folder_rel) = paths::resolve_inside_root(&args.folder, &root, &cwd)?;

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

    let tmp = tempdir().context("failed to create temporary directory")?;
    let shallow = args.git_tag.is_none();
    git::cli::clone(&link, tmp.path(), shallow)?;
    if let Some(ref tag) = args.git_tag {
        git::cli::checkout(tmp.path(), tag.clone())?;
    }

    let commit_hash = git::cli::commit_hash_of(tmp.path())?;

    // Copy + register. Anything that fails after copy_dir triggers a rollback
    // so we never leave on-disk state without a matching config entry.
    filesystem::copy_dir(tmp.path(), &folder_abs)?;

    let manifest_path = paths::cache_path(&root, &repo_name);

    let result = (|| -> Result<()> {
        let manifest = Manifest::build_from_path(&folder_abs, commit_hash.clone())?;
        manifest.save(&manifest_path)?;
        config.insert(
            repo_name.clone(),
            EmbdEntry {
                remote: link,
                commit_hash,
                folder: folder_rel,
                allow_untracked: args.allow_untracked,
            },
        )?;
        config.save(&config_path)?;
        Ok(())
    })();

    if let Err(e) = result {
        // If there was an error, try to rollback to the previous state.
        rollback(&folder_abs, folder_existed, args.allow_untracked);
        // The manifest may or may not have been written; remove it regardless.
        let _ = std::fs::remove_file(&manifest_path);
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
