use anyhow::Result;

use crate::color;
use crate::commands::common::select_entries;
use crate::config;
use crate::lockfile::{self, EntryReport, EntryState, FileChange, Lockfile};
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
    let lock = Lockfile::load_or_default(&paths::lock_path(&root))?;
    let selected = select_entries(&config, &args.names)?;

    let mut any_drift = false;
    for (name, entry) in selected {
        let report = lockfile::inspect_entry(&root, name, entry, lock.get(name));
        any_drift |= report.has_drift();
        print_report(&report, args.quiet);
    }

    if any_drift {
        std::process::exit(1);
    }
    Ok(())
}

pub(crate) fn print_report(report: &EntryReport, quiet: bool) {
    use anstream::println;

    let header = color::header(&format!("{} ({})", report.name, report.folder.display()));

    match report.state {
        EntryState::FolderMissing => {
            println!("{header}: {}", color::bad("folder missing"));
            return;
        }
        EntryState::Missing => {
            println!("{header}: {}", color::bad("missing from lock file"));
            return;
        }
        EntryState::Compared => {}
    }

    let summary = if let Some((local, wanted)) = &report.stale {
        color::warn(&format!("stale (folder at {local}, config wants {wanted})"))
    } else if report.changes.is_empty() {
        color::ok("clean")
    } else {
        color::warn("drift")
    };
    println!("{header}: {summary}");

    if quiet {
        return;
    }
    for change in &report.changes {
        let mrkr = change.as_marker();
        match change {
            FileChange::Modified(p) => println!("  {}  {p}", color::marker(mrkr)),
            FileChange::Deleted(p) => println!("  {}  {p}", color::marker(mrkr)),
            FileChange::Untracked(p) => {
                if report.allow_untracked {
                    println!("  {}  {p} (untracked, allowed)", color::marker(mrkr));
                } else {
                    println!("  {}  {p} (untracked)", color::marker(mrkr));
                }
            }
            FileChange::Symlink(p) => println!("  {}  {p} (symlink)", color::marker(mrkr)),
        }
    }
}
