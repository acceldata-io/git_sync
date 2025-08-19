use clap::{Args, ArgGroup,Parser, Subcommand};
use std::path::PathBuf;


#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct AppArgs {
    ///Github Personal Access Token
    #[arg(short = 't', long, env = "GITHUB_TOKEN")]
    pub token: Option<String>,

    /// Path to config file
    #[arg(short = 'f', long)]
    pub file: Option<PathBuf>,
    
    /// The types of repositories to use. Valid options are public, private, or forks.
    #[arg(short, long, default_value = "forks")]
    pub repository_type: String,

    #[command(subcommand)]
    pub command: Command,

}

#[derive(Args, Clone, Debug)]
#[command(group(
    ArgGroup::new("target")
    .required(true)
    .args(&["all", "repository"])
))]
pub struct CreateTagCommand {
    /// The new tag's name
    #[arg(short, long, required = true)]
    tag: String,
    /// The target branch
    #[arg(short, long, required = true)]
    branch: String,
    /// The url of the repository to create the tag in. If --all is specified, this is
    /// not a valid option.
    #[arg(short, long, required = false)]
    repository: String,
    /// Create this tag for all configured repositories, all using the same branch.
    /// If --repository is specified, this is not a valid option.
    #[arg(short, long,default_value_t = false)]
    all: bool,
}
#[derive(Args, Clone, Debug)]
pub struct CompareTagCommand {
    /// The base repository to compare against
    #[arg(short, long, required = false)]
    repository: Option<String>,
    #[arg(short, long, required = false)]
    parent: Option<String>,
    #[arg(short, long, default_value_t = false)]
    all: bool,
}
 impl CompareTagCommand {
    pub fn validate(&self) -> Result<(), String> {
        match (&self.repository, &self.parent, self.all) {
            (Some(_), Some(_), _) => Ok(()),
            (None, None, true) => Ok(()),
            _ => Err("Must specify either --repository or --parent, or use --all".to_string())
        }
    }
}

#[derive(Args, Clone, Debug)]
#[command(group(
    ArgGroup::new("sync")
    .required(true)
    .args(&["all", "repository"])
))]
pub struct SyncRepoCommand {
    /// The repository to sync
    #[arg(short, long, required = false)]
    repository: Option<String>,
    #[arg(short, long, default_value_t = false)]
    all: bool,
}
#[derive(Args, Clone, Debug)]
#[command(
    group(
        ArgGroup::new("target")
        .required(true)
        .args(&["all", "repository"])
    ),
    group(
        ArgGroup::new("check")
        .required(true)
        .multiple(true)
        .args(&["license", "protected"])
    )
)]
pub struct CheckRepoCommand {
    /// The repository to check
    #[arg(short, long, required = false)]
    repository: Option<String>,
    #[arg(short, long, default_value_t = false)]
    all: bool,
    #[arg(short, long, default_value_t = false)]
    license: bool,
    #[arg(short, long, default_value_t = false)]
    protected: bool
}

#[derive(Subcommand, Clone, Debug)]
pub enum TagCommand {
    /// Sync tags
    //Sync(SyncTagCommand),
    /// Compare tags between repositories
    Compare(CompareTagCommand),
    /// Create a new tag
    Create(CreateTagCommand),
}

#[derive(Subcommand, Clone, Debug)]
pub enum RepoCommand {
    /// Sync repoositories
    Sync(SyncRepoCommand),
    Check(CheckRepoCommand),
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage tags
    #[command(arg_required_else_help = true)]
    Tags {
        /// Repository to act upon
        #[arg(short, long, global = true)]
        repository: Option<String>,
        /// Command to run
        #[command(subcommand)]
        cmd: Option<TagCommand>,
    },

    /// Generate a default config
    InitConfig {
        /// Path to save the config file
        #[arg(short, long)]
        path: Option<PathBuf>,

        /// Overwrite existing config file if it existing
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Manage repositories
    #[command(arg_required_else_help = true)]
    Repos {
        /// Sync a repository with its upstream parent
        #[arg(short, long, global = true)]
        repository: Option<String>,
        /// Sync all configured repositories
        #[command(subcommand)]
        cmd: Option<RepoCommand>,
    },
}

pub fn parse_args() -> AppArgs {
    let app = AppArgs::try_parse();
    let app = match app {
        Ok(app) => app,
        Err(err) => {
            err.print().unwrap();
            std::process::exit(1);
        }
    };
    if let Command::Tags { repository: Some(_),cmd: Some(TagCommand::Compare( compare)) } = &app.command {
        if let Err(e) = compare.validate() {
            eprintln!("Error validating compare command: {e}");

            std::process::exit(1);
        };
    }
    app
}
