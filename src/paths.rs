//! This module helps with getting standard paths for use with the `embd` CLI

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::git;

const EMBD_FOLDER: &str = ".embd";
const CONFIG_FILE: &str = "config.toml";

pub(crate) fn find_git_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    git::cli::find_git_root(cwd)
}

pub(crate) fn project_folder(root_path: &Path) -> PathBuf {
    root_path.join(EMBD_FOLDER)
}

pub(crate) fn config_path(root_path: &Path) -> PathBuf {
    project_folder(root_path).join(CONFIG_FILE)
}
