use anyhow::{Context, Result};

use crate::color;
use crate::commands::common::select_entries;
use crate::config::{self, EntryReport, EntryState, FileChange};
use crate::paths;

#[derive(clap::Args, Debug)]
pub(crate) struct StatusArgs {
    /// Optional folder paths to check. When empty, every embed in the project is checked.
    folders: Vec<String>,
    /// Suppress per-file rows; print only one-line summaries per entry.
    #[clap(short, long)]
    quiet: bool,
}

pub(crate) fn execute(args: StatusArgs) -> Result<()> {
    let root = paths::find_git_root()?;
    let cwd = std::env::current_dir().context("failed to read current directory")?;
    let selected = select_entries(&root, &args.folders, &cwd)?;

    let mut any_drift = false;
    for (folder, entry) in &selected {
        let report = config::inspect_entry(&root, folder, entry);
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

    let header = color::header(&report.folder.display().to_string());

    match report.state {
        EntryState::FolderMissing => {
            println!("{header}: {}", color::bad("folder missing"));
            return;
        }
        EntryState::Compared => {}
    }

    let summary = if report.changes.is_empty() {
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
