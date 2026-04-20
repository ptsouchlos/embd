//! This module helps with getting standard paths for use with the `embd` CLI

use std::path::PathBuf;

const EMBD_FOLDER: &str = ".embd";

pub(crate) fn project_folder(root_path: PathBuf) -> PathBuf {
    let mut output = root_path;
    output.push(PathBuf::from(EMBD_FOLDER));
    output
}
