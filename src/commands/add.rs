use std::path::PathBuf;

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

pub(crate) fn execute(_args: AddArgs) {
    todo!("Not implemented.");
}
