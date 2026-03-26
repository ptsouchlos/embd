use clap::Parser;

use crate::commands::add::AddArgs;
mod commands;

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
    let opts = Options::parse();
    match opts.command {
        Command::Add(add_args) => commands::add::execute(add_args),
    }
}
