/*
Licensed to the Apache Software Foundation (ASF) under one
or more contributor license agreements.  See the NOTICE file
distributed with this work for additional information
regarding copyright ownership.  The ASF licenses this file
to you under the Apache License, Version 2.0 (the
"License"); you may not use this file except in compliance
with the License.  You may obtain a copy of the License at

  http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing,
software distributed under the License is distributed on an
"AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
KIND, either express or implied.  See the License for the
specific language governing permissions and limitations
under the License.
*/
use clap::{ArgGroup, Args, Parser, Subcommand};
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

/// Options for the 'create' tag command
#[derive(Args, Clone, Debug)]
#[command(group(
    ArgGroup::new("target")
    .required(true)
    .args(&["all", "repository"])
))]
pub struct CreateTagCommand {
    /// The new tag's name
    #[arg(short, long, required = true)]
    pub tag: String,
    /// The target branch
    #[arg(short, long, required = true)]
    pub branch: String,
    /// The url of the repository to create the tag in. If --all is specified, this is
    /// not a valid option.
    #[arg(short, long, required = false)]
    pub repository: Option<String>,
    /// Create this tag for all configured repositories, all using the same branch.
    /// If --repository is specified, this is not a valid option.
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
}
/// Options for the 'comparison' tag command
#[derive(Args, Clone, Debug)]
pub struct CompareTagCommand {
    /// The base repository to compare against
    #[arg(short, long, required = false)]
    pub repository: Option<String>,
    #[arg(short, long, required = false)]
    pub parent: Option<String>,
    /// Apply to all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
}
impl CompareTagCommand {
    /// Validate that both --repository and --parent are specified, or that --all is.
    /// We have to use a user defined function for this since clap doesn't have a way
    /// to enforce this through the type system or macros.
    pub fn validate(&self) -> Result<(), String> {
        match (&self.repository, &self.parent, self.all) {
            (Some(_), Some(_), _) => Ok(()),
            (None, None, true) => Ok(()),
            _ => Err("Must specify --repository and --parent, or use --all".to_string()),
        }
    }
}

#[derive(Args, Clone, Debug)]
#[command(group(
    ArgGroup::new("target")
    .required(true)
    .args(&["all", "repository"])
))]
pub struct DeleteTagCommand {
    /// The target repository. Not a valid option if 'all' is set
    #[arg(short, long, required = false)]
    pub repository: Option<String>,
    /// The tag to delete
    #[arg(short, long, required = true)]
    pub tag: String,
    /// Apply to all configured repositories. Not a valid if 'repository' is set
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
}

/// Define the arguments for the 'sync' command for repositories
#[derive(Args, Clone, Debug)]
#[command(group(
    ArgGroup::new("sync")
    .required(true)
    .args(&["all", "repository"])
))]
pub struct SyncRepoCommand {
    /// The repository to sync
    #[arg(short, long, required = false)]
    pub repository: Option<String>,
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
}
/// Define the arguments for the 'check' repository command
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
    pub repository: Option<String>,
    /// Check all repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
    /// Enable checking the license of target repositories
    #[arg(short, long, default_value_t = false)]
    pub license: bool,
    /// Enable checking the protected status of the main branch for target repositories
    #[arg(short, long, default_value_t = false)]
    pub protected: bool,
}

/// Define the arguments for the 'delete' branch command
#[derive(Args, Clone, Debug)]
#[command(
    group(
        ArgGroup::new("target")
        .required(true)
        .args(&["all", "repository"])
    )
)]
pub struct DeleteBranchCommand {
    /// Delete a branch from a repository
    #[arg(short, long, required = false)]
    pub repository: Option<String>,
    /// Branch to delete
    #[arg(short, long, required = true)]
    pub branch: String,
    /// Delete the specified branch for all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
}

#[derive(Args, Clone, Debug)]
#[command(
    group(
        ArgGroup::new("target")
        .required(true)
        .args(&["all", "repository"])
    )
)]
pub struct CreateBranchCommand {
    /// Create a branch in this repository
    #[arg(short, long, required = false)]
    pub repository: Option<String>,
    /// New branch to create
    #[arg(short, long, required = true)]
    pub new_branch: String,
    /// The base branch for the new branch
    #[arg(short, long, required = true)]
    pub base_branch: String,
    /// Create the branch for all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
}

/// Define all the valid commands for acting on Tags
#[derive(Subcommand, Clone, Debug)]
pub enum TagCommand {
    /// Sync tags
    //Sync(SyncTagCommand),
    /// Compare tags between repositories
    Compare(CompareTagCommand),
    /// Create a new tag
    Create(CreateTagCommand),
    /// Delete a tag
    Delete(DeleteTagCommand),
}

/// Define all the valid commands for acting on repositories
#[derive(Subcommand, Clone, Debug)]
pub enum RepoCommand {
    /// Sync repoositories
    Sync(SyncRepoCommand),
    /// Check repositories
    Check(CheckRepoCommand),
}

/// Define all the valid commands for acting on branches
#[derive(Subcommand, Clone, Debug)]
pub enum BranchCommand {
    /// Delete a branch in repositories
    Delete(DeleteBranchCommand),
    /// Create a branch for repositories
    Create(CreateBranchCommand),
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage branches
    #[command(arg_required_else_help = true)]
    Branch {
        /// Manage a branches for this repository
        #[arg(short, long, global = true)]
        repository: Option<String>,
        /// Manage branches
        #[command(subcommand)]
        cmd: BranchCommand,
    },
    /// Generate a default config
    Config {
        /// Path to save the config file.
        #[arg(short, long)]
        path: Option<PathBuf>,

        /// Overwrite existing config file if it exists
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Manage repositories
    #[command(arg_required_else_help = true)]
    Repo {
        /// Sync a repository with its upstream parent
        #[arg(short, long, global = true)]
        repository: Option<String>,
        /// Sync all configured repositories
        #[command(subcommand)]
        cmd: RepoCommand,
    },
    /// Manage tags
    #[command(arg_required_else_help = true)]
    Tag {
        /// Repository to act upon
        #[arg(short, long, global = true, required = false)]
        repository: Option<String>,
        /// Command to run
        #[command(subcommand)]
        cmd: TagCommand,
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
    if let Command::Tag {
        repository: Some(_),
        cmd: TagCommand::Compare(compare),
    } = &app.command
    {
        if let Err(e) = compare.validate() {
            eprintln!("Error validating compare command: {e}");

            std::process::exit(1);
        };
    }
    app
}
