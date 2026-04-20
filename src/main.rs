use anyhow::Result;
use clap::Parser;

use crate::commands::add::AddArgs;
mod commands;
mod config;
mod paths;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
enum Command {
    #[command(about, long_about = "Add a new submodule/embed.")]
    Add(AddArgs),
}

#[derive(Parser, Debug)]
struct Options {
    #[clap(subcommand)]
    command: Command,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let opts = Options::parse();
    match opts.command {
        Command::Add(add_args) => commands::add::execute(add_args)?,
    }
    Ok(())
}
