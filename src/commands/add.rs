use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tempfile::tempdir;

use crate::config::{Config, EmbdEntry};
use crate::{filesystem, git, paths};

#[derive(clap::Args, Debug)]
pub(crate) struct AddArgs {
    #[clap(short, long, help = "Name of the entry")]
    name: String,
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
}

pub(crate) fn execute(args: AddArgs) -> Result<()> {
    let root = paths::find_git_root()?;
    let config_path = paths::config_path(&root);

    let mut config = match Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
            {
                Config::default()
            } else {
                return Err(e);
            }
        }
    };

    if config.contains(&args.name) {
        bail!(
            "an embed named '{}' already exists in the config",
            args.name
        );
    }

    if args.folder.is_dir() && !args.allow_untracked {
        let mut entries = std::fs::read_dir(&args.folder)
            .with_context(|| format!("failed to read directory {}", args.folder.display()))?;
        if entries.next().is_some() {
            bail!(
                "folder '{}' is non-empty; use --allow-untracked to proceed anyway",
                args.folder.display()
            );
        }
    }

    let tmp = tempdir().context("failed to create temporary directory")?;
    git::cli::clone(&args.link, tmp.path())?;
    let commit_hash = git::cli::commit_hash_of(tmp.path())?;
    filesystem::copy_dir(tmp.path(), &args.folder)?;

    config.insert(
        args.name,
        EmbdEntry {
            remote: args.link,
            commit_hash,
            folder: args.folder,
        },
    )?;
    config.save(&config_path)?;

    Ok(())
}
