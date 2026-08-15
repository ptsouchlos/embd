use std::path::Path;

mod entry;
mod file_locks;
mod hash;
mod metadata;
mod scan;
mod store;
mod walk;

pub use entry::EmbdEntry;
pub use file_locks::FileLocks;
pub use metadata::Metadata;
pub use store::{Config, load_or_default};

pub(crate) use hash::hash_file;
pub(crate) use scan::{EntryReport, EntryState, FileChange, inspect_entry, scan_folder};

/// Convert a relative path to the string key used in the manifest. Paths are
/// stored with forward slashes regardless of host OS so manifests stay portable.
pub(crate) fn path_to_key(relative: &Path) -> String {
    let mut parts = Vec::new();
    for component in relative.components() {
        parts.push(component.as_os_str().to_string_lossy().into_owned());
    }
    parts.join("/")
}
