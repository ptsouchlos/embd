use anyhow::Result;
use clap::Parser;

use crate::commands::add::AddArgs;
use crate::commands::status::StatusArgs;
use crate::commands::update::UpdateArgs;
mod cache;
mod commands;
mod config;
mod filesystem;
mod git;
mod paths;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
enum Command {
    #[command(about, long_about = "Add a new submodule/embed.")]
    Add(AddArgs),
    #[command(about, long_about = "Show drift between embeds and the config.")]
    Status(StatusArgs),
    #[command(about, long_about = "Apply pinned embeds to disk; optionally bump pins with --rev.")]
    Update(UpdateArgs),
}

#[derive(Parser, Debug)]
struct Options {
    #[clap(subcommand)]
    command: Command,
}

/// Main function that runs the CLI. It parses the arguments and dispatches to the appropriate command handler.
fn run() -> Result<()> {
    let opts = Options::parse();
    match opts.command {
        Command::Add(add_args) => commands::add::execute(add_args)?,
        Command::Status(status_args) => commands::status::execute(status_args)?,
        Command::Update(update_args) => commands::update::execute(update_args)?,
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
