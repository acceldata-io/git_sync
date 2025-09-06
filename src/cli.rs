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
use crate::utils::pr::MergeMethod;
use clap::{ArgGroup, Args, CommandFactory, Parser, Subcommand, ValueEnum};
use std::fmt;
use std::path::PathBuf;
/// `git_sync` is an application for managing multiple github repositories at once.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct AppArgs {
    ///Github Personal Access Token
    #[arg(short, long, env = "GITHUB_TOKEN")]
    pub token: Option<String>,

    /// Path to config file
    #[arg(short, long, global = true, env = "CONFIG_FILE")]
    pub file: Option<PathBuf>,

    /// The types of repositories to use for any command that targets configured repositories
    #[arg(short, long, default_value = "fork")]
    pub repository_type: RepositoryType,

    /// The command that will get run
    #[command(subcommand)]
    pub command: Command,

    /// Make output quiet. This is useful when not running in interactive mode
    #[arg(short, long, default_value_t = false, global = true)]
    pub quiet: bool,

    /// Verbose output
    #[arg(long, default_value_t = false, global = true)]
    pub verbose: bool,

    /// The maximum number of parallel tasks to run
    #[arg(short = 'j', long, global = true, value_parser = clap::value_parser!(u32).range(1..64))]
    pub jobs: Option<usize>,
}

/// Valid options for `repository_type` for cli arguments
#[derive(Copy, Clone, PartialEq, Eq, Debug, ValueEnum)]
pub enum RepositoryType {
    Public,
    Private,
    Fork,
    All,
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
            (Some(_), Some(_), _) | (None, None, true) => Ok(()),
            _ => Err("Must specify --repository and --parent, or use --all".to_string()),
        }
    }
}
/// Defines the arguments for the 'delete' tag subcommand
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

#[derive(Args, Clone, Debug)]
#[command(group(
    ArgGroup::new("target")
    .required(true)
    .args(&["all", "repository"])
))]
pub struct SyncTagCommand {
    /// The Repository to sync tags for. Not valid if '--all' is set
    #[arg(short, long, required = false)]
    pub repository: Option<String>,
    /// Sync tags for all configured repositories. Not valid if '--repository' is set
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
    /// Disable syncing annotated tags. Syncing annotated tags requires git to be setup with commit access
    /// to the repositories you are attempting to manage.
    #[arg(short, long, default_value_t = false)]
    pub without_annotated: bool,
}

/// Sync a repository. If the repository is already up to date, no error is reported.
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
        .args(&["license", "protected", "old_branches"])
    ),
    long_about = "Check repositories for various conditions, such as the license used, branch protection status, and stale branches. \
        You can target a single repository or all configured repositories. Multiple checks can be run is a single commmand.",
    after_help="\
EXAMPLES:
    # Check all repositories for license and branch protection
    git_sync repo check --all --license --protected --branch \"ODP-main\"
    # Check a specific repository for old branches inactive for 60 days matching a specific pattern. The pattern is optional, and if not provided, all branches will be checked.
    git_sync repo check --repository my-repo --old-branches --days-ago --branch-filter \"^ODP\"

NOTES:
    - At least one of --license, --protected, or --old-branches must be specified.
    - Use --all to apply checks to all configured repositories, or --repository option to target one repository. You cannot use both at the same time.
        "

)]
/// Check repositories for various conditions
#[allow(clippy::struct_excessive_bools)]
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
    /// Specify the branch to check. Required when '--protected' is enabled
    #[arg(long, required_if_eq("protected", "true"))]
    pub branch: Option<String>,
    #[arg(short, long, default_value_t = false)]
    /// Enable checking for old branches in target repository
    pub old_branches: bool,
    /// Number of days of inactivity after which a branch is flagged
    #[arg(short, long, default_value_t = 30)]
    pub days_ago: i64,
    /// Regex that can be used to filter branches
    #[arg(short, long, required = false)]
    pub branch_filter: Option<String>,
}
#[derive(Args, Clone, Debug)]
#[command(
    group(
        ArgGroup::new("target")
        .required(true)
        .args(&["all", "repository"])
    )
)]
/// Open a PR to merge <branch> into <`base_branch`>
pub struct CreatePRCommand {
    /// Repository for which you are creating a PR
    #[arg(short, long, required = false)]
    pub repository: Option<String>,
    /// Create a PR for all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
    /// The name of the branch where your changes are implemented
    #[arg(long, required = true)]
    pub head: String,
    /// The name of the branch you want the changes pulled into
    #[arg(short, long, required = true)]
    pub base: String,
    /// The title for the PR
    #[arg(short, long)]
    pub title: String,

    /// The body for the PR
    #[arg(long)]
    pub body: Option<String>,

    /// Attempt to merge the PR after creating it. This will only succeed if the PR is mergeable and
    /// has no conflicts.
    #[arg(long, default_value_t = false)]
    pub merge: bool,
    /// Title for the automatic commit message
    #[arg(long)]
    pub merge_title: Option<String>,
    /// Extra detail to append to automatic commit message
    #[arg(long)]
    pub merge_body: Option<String>,
    /// The method to use when merging the PR
    #[arg(long, value_enum, default_value = "merge")]
    pub merge_method: MergeMethod,
    /// SHA that the pull request head must match to permit merging
    #[arg(long, required = false)]
    pub sha: Option<String>,
    /// A list of reviewers to request a review from
    #[arg(long, required = false)]
    pub reviewers: Option<Vec<String>>,
}
#[derive(Args, Clone, Debug)]
#[command(
    group(
        ArgGroup::new("target")
        .required(true)
        .args(&["all", "repository"])
    )
)]
pub struct ClosePRCommand {
    /// Repository for which you are closing a PR
    #[arg(short, long, required = false)]
    pub repository: Option<String>,
    /// Close a PR for all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
    /// The PR number
    #[arg(short, long, required = true)]
    pub id: u64,

    /// The base branch of the PR to close
    #[arg(short, long, required = true)]
    pub base_branch: String,
}

/// Merge a PR. This will only work if the PR is mergeable and has no conflicts.
#[derive(Args, Clone, Debug)]
#[command(
    group(
        ArgGroup::new("target")
        .required(true)
        .args(&["all", "repository"])
    )
)]
pub struct MergePRCommand {
    /// Repository for which you are merging a PR
    #[arg(short, long, required = false)]
    pub repository: Option<String>,
    /// The PR number
    #[arg(short, long, required = true, name = "pull_number")]
    pub id: u64,

    /// The base branch of the PR to close
    #[arg(short, long, required = true)]
    pub base_branch: String,
    /// The method to use when merging the PR
    #[arg(short, long, value_parser = ["merge", "squash", "rebase"], default_value = "squash")]
    pub merge_method: String,

    /// Title for the automatic commit message
    #[arg(long, required = false)]
    pub commit_title: Option<String>,

    /// Extra detail to append to the automatic commit message
    #[arg(long, required = false)]
    pub commit_message: Option<String>,

    /// SHA that the pull request head must match to permit merging
    #[arg(long, required = false)]
    pub sha: Option<String>,
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

/// Create a new branch. This will return an error if the branch already exists.
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

#[derive(Args, Clone, Debug)]
#[command(
    group(
        ArgGroup::new("target")
        .required(true)
        .args(&["all", "repository"])
    ),
)]
pub struct CreateReleaseCommand {
    /// The tag or branch to base the release off of
    #[arg(short, long, required = true)]
    pub current_release: String,
    /// The previous release tag or branch. This is used to generate the changelog
    #[arg(short, long, required = true)]
    pub previous_release: String,
    /// The repository for which to create the release
    #[arg(short, long, required = false)]
    pub repository: Option<String>,

    /// The name of the release. If not specified, the tag name will be used
    #[arg(long, required = false)]
    pub release_name: Option<String>,

    /// Set this release to the latest
    #[arg(short, long, default_value_t = MakeLatest::True)]
    pub latest: MakeLatest,
    /// Specify whether this release should use 'legacy' to determine if it's the newest release.
    /// 'legacy' specifies that the latest release should be determined using creation date and a
    /// higher version number

    /// Create this release for all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,

    /// Disable release note generation. This decreases the number of api calls
    #[arg(short, long, default_value_t = false)]
    pub no_release_notes: bool,
}
/// All valid commands concerning PRs
#[derive(Subcommand, Clone, Debug)]
pub enum PRCommand {
    /// Create a new PR
    Open(CreatePRCommand),
}
/// All valid commands concerning releases
#[derive(Subcommand, Clone, Debug)]
pub enum ReleaseCommand {
    /// Create a new release
    Create(CreateReleaseCommand),
    // Delete a release
    //Delete(DeleteReleaseCommand)
}
/// Valid options that can be passed to `make_latest` in the github api.
/// True makes this the latest release, false makes it not the latest release,
/// Legacy looks at both the date, and the semantic version to decide if this should be the latest
#[derive(Copy, Clone, PartialEq, Eq, Debug, ValueEnum)]
pub enum MakeLatest {
    True,
    False,
    Legacy,
}
/// Implement `fmt::Display` so this type can be printed to the cli
impl fmt::Display for MakeLatest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MakeLatest::True => write!(f, "true"),
            MakeLatest::False => write!(f, "false"),
            MakeLatest::Legacy => write!(f, "legacy"),
        }
    }
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
    /// Sync tags for a forked repository with its parent
    Sync(SyncTagCommand),
}

/// Define all the valid commands for acting on repositories
#[derive(Subcommand, Clone, Debug)]
pub enum RepoCommand {
    /// Sync repositories
    Sync(SyncRepoCommand),
    /// Check repositories for various conditions, such as branch protection being enabled, stale
    /// branches, etc.
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
/// The top-level command enum for the CLI
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage branches
    #[command(arg_required_else_help = true)]
    Branch {
        /// Manage branches
        #[command(subcommand)]
        cmd: BranchCommand,
    },
    /// Generate a default config
    #[command(arg_required_else_help = true)]
    Config {
        /// Path to save the config file.
        #[arg(short, long)]
        file: Option<PathBuf>,

        /// Overwrite existing config file if it exists
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Manage repositories
    #[command(arg_required_else_help = true)]
    Repo {
        /// Sync all configured repositories
        #[command(subcommand)]
        cmd: RepoCommand,
    },
    /// Manage tags
    #[command(arg_required_else_help = true)]
    Tag {
        /// Command to run
        #[command(subcommand)]
        cmd: TagCommand,
    },
    #[command(arg_required_else_help = true)]
    Release {
        /// Command to run
        #[command(subcommand)]
        cmd: ReleaseCommand,
    },
    #[command(arg_required_else_help = true)]
    PR {
        /// Manage PRs
        #[command(subcommand)]
        cmd: PRCommand,
    },
    /// Generate completions or a manpage. This command is hidden by default since it should really
    /// done at build time
    #[command(hide = true)]
    Generate {
        /// What to generate. Can be shell completion for bash, zsh, or fish; or manpages.
        #[arg(long, value_parser = ["bash", "zsh", "fish", "man"])]
        kind: String,
        /// An optional output path. If not specified, the current directory will be used instead
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
}
/// Parse the command line arguments and validate them.
/// This function uses clap to generate the `AppArgs` struct,
/// and then runs a small amount of validation on it that can't be
/// enforced by the type system or macros.
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
        cmd: TagCommand::Compare(compare),
    } = &app.command
    {
        if let Err(e) = compare.validate() {
            eprintln!("Error validating compare command: {e}");

            std::process::exit(1);
        }
    }
    app
}

pub fn cli() -> clap::Command {
    AppArgs::command()
}
