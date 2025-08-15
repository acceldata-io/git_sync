use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    ///Github Personal Access Token
    #[arg(short = 't', long, env = "GITHUB_TOKEN")]
    pub token: Option<String>,

    /// Path to config file
    #[arg(short = 'f', long)]
    pub file: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum CompareCommand {
    /// Compare tags between a fork and its parent
    Single {
        /// The owner of the fork repository
        #[arg(short, long)]
        owner: String,
        /// The name of the fork repository
        #[arg(short, long)]
        repo: String,
    },
    // compare all configured repos
    All,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    Compare {
        #[command(subcommand)]
        cmd: CompareCommand,
    },

    /// Generate a default config
    InitConfig {
        /// Path to save the config file
        #[arg(short, long)]
        path: Option<PathBuf>,

        /// Overwrite existing config file if it existing
        #[arg(long)]
        force: bool,
    },
}

pub fn parse_args() -> Args {
    Args::parse()
}
