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

/// `git_sync` is an application for managing multiple GitHub repositories at once.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None,
    after_help="\
NOTES:
    - When using --repository-type, make sure that whatever operation you are running can be applied across all of the repositories in the configured group.",
)]
#[allow(clippy::struct_excessive_bools)]
pub struct AppArgs {
    /// GitHub Personal Access Token
    #[arg(short, long, env = "GITHUB_TOKEN")]
    pub token: Option<String>,

    /// Path to config file
    #[arg(short, long, global = true, env = "CONFIG_FILE")]
    pub file: Option<PathBuf>,

    /// The types of repositories to use for any command that targets configured repositories.
    /// Warning: selecting 'all' will include every repository from every category
    #[arg(long, global = true, default_value = "fork")]
    pub repository_type: RepositoryType,

    /// The name of the custom repository group to use. Required if `repository_type` is set to
    /// `custom`
    #[arg(
        short = 'g',
        long = "group",
        global = true,
        required_if_eq("repository_type", "custom")
    )]
    pub repository_group: Option<String>,

    /// The command that will get run
    #[command(subcommand)]
    pub command: Command,

    /// Make output quiet. This is useful when not running in interactive mode. If Slack is
    /// enabled, this will silence some success messages to slack.
    #[arg(short, long, default_value_t = false, global = true)]
    pub quiet: bool,

    /// Verbose output. Currently only checks and reports your api usage
    #[arg(long, default_value_t = false, global = true)]
    pub verbose: bool,

    /// The maximum number of parallel tasks to run.
    #[arg(short = 'j', long, global = true, value_parser = validate_jobs)]
    pub jobs: Option<usize>,

    /// Also process repositories that are forks that do not have the parent repository set in a way that
    /// GitHub understands.
    #[arg(long, default_value_t = false, global = true)]
    pub with_fork_workaround: bool,

    /// Enable sending the results of the operation to a Slack channel using the
    /// configured webhook in git-manage.toml
    #[cfg(feature = "slack")]
    #[arg(short, long, global = true, default_value_t = false)]
    pub slack: bool,

    /// When true, runs through and reports activity without pushing any commits.
    #[arg(long, global = true, default_value_t = false)]
    pub dry_run: bool,

    /// Override the slack webhook url from the config file
    #[cfg(feature = "slack")]
    #[arg(long, global = true, env = "SLACK_WEBHOOK")]
    pub slack_webhook: Option<String>,
}

/// Validate that the maximum number of parallel jobs is between 1 and 64.
/// Strictly speaking, there isn't a reason that this couldn't be higher, but
/// there isn't much point in allowing more jobs than cpu cores available
fn validate_jobs(s: &str) -> Result<usize, String> {
    let parsed: usize = s
        .parse()
        .map_err(|_| format!("'{s}' is not a valid positive integer"))?;
    if (1..=64).contains(&parsed) {
        Ok(parsed)
    } else {
        Err("Jobs must be between 1 and 64".to_string())
    }
}

// === Enums ===

/// Valid options for `repository_type` for cli arguments
#[derive(Copy, Clone, PartialEq, Eq, Debug, ValueEnum)]
pub enum RepositoryType {
    Public,
    Private,
    Fork,
    All,
    Custom,
}

/// Valid options that can be passed to `make_latest` in the github api.
#[derive(Copy, Clone, PartialEq, Eq, Debug, ValueEnum)]
pub enum MakeLatest {
    True,
    False,
    Legacy,
}

/// Choose where to store backups. When the aws feature is not enabled, only `Local` is a valid
/// option
#[derive(Copy, Clone, PartialEq, Eq, Debug, ValueEnum)]
pub enum BackupDestination {
    Local,
    #[cfg(feature = "aws")]
    S3,
}

impl fmt::Display for BackupDestination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BackupDestination::Local => write!(f, "local"),
            #[cfg(feature = "aws")]
            BackupDestination::S3 => write!(f, "s3"),
        }
    }
}

impl fmt::Display for MakeLatest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MakeLatest::True => write!(f, "true"),
            MakeLatest::False => write!(f, "false"),
            MakeLatest::Legacy => write!(f, "legacy"),
        }
    }
}

// === Command Structs ===

// --- Tag Commands ---

/// Options for the 'create' tag command
#[derive(Args, Clone, Debug)]
#[command(group(
    ArgGroup::new("target")
    .required(true)
    .args(&["all", "repository"])
))]
pub struct CreateTagCommand {
    /// The new tag's name
    #[arg(short, long)]
    pub tag: String,
    /// The target branch
    #[arg(short, long)]
    pub branch: String,
    /// The url of the repository to create the tag in. If --all is specified, this is
    /// not a valid option.
    #[arg(short, long)]
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
    #[arg(short, long)]
    pub repository: Option<String>,
    #[arg(short, long)]
    pub parent: Option<String>,
    /// Apply to all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
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
    #[arg(short, long)]
    pub repository: Option<String>,
    /// The tag to delete
    #[arg(short, long)]
    pub tag: String,
    /// Apply to all configured repositories. Not a valid if 'repository' is set
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
}

/// Sync tags for a forked repository with its parent
#[derive(Args, Clone, Debug)]
#[command(group(
    ArgGroup::new("target")
    .required(true)
    .args(&["all", "repository"])
))]
pub struct SyncTagCommand {
    /// The Repository to sync tags for. Not valid if '--all' is set
    #[arg(short, long)]
    pub repository: Option<String>,
    /// Sync tags for all configured repositories. Not valid if '--repository' is set
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
    /// Sync annotated tags. Enabling this requires that git is set up in your environment, with
    /// read/write permissions for the repositories you are syncing.
    #[arg(short, long, default_value_t = false)]
    pub with_annotated: bool,
}
/// Show tags for a single repository or all repositories, with optional filtering by regex.
#[derive(Args, Clone, Debug)]
#[command(group(
    ArgGroup::new("target")
    .required(true)
    .args(&["all", "repository"])
))]
pub struct ShowTagCommand {
    /// The repository to show tags for. Not valid if '--all' is set
    #[arg(short, long)]
    pub repository: Option<String>,
    /// The regex filter to apply to tag names
    #[arg(short = 'l', long, default_value = "")]
    pub filter: String,
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
}

// --- Repo Commands ---

/// Sync a repository. If the repository is already up to date, no error is reported.
#[derive(Args, Clone, Debug)]
#[command(group(
    ArgGroup::new("sync")
        .required(true)
        .args(&["all", "repository"]),
    ),
    group(
        ArgGroup::new("target")
        .args(&["branch", "recursive"])
    ),
    group(
        ArgGroup::new("branch_conflict")
        .args(&["branch", "all"])
))]
#[allow(clippy::struct_excessive_bools)]
pub struct SyncRepoCommand {
    /// The repository to sync
    #[arg(short, long)]
    pub repository: Option<String>,
    /// Sync all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
    /// Sync all common branches between the fork and its parent
    #[arg(long, default_value_t = false)]
    pub recursive: bool,
    /// Force the sync. This is required when using --all and --recursive since this operation can
    /// be very long
    #[arg(long, default_value_t = false)]
    pub force: bool,
    /// Sync a specific branch. Not a valid option when using --all or --recursive
    #[arg(short, long)]
    pub branch: Option<String>,
}

/// Check repositories for various conditions
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
        You can target a single repository or all configured repositories. Multiple checks can be run is a single command.",
    after_help="\
EXAMPLES:
    # Check all repositories for license and branch protection
    git_sync repo check --all --license --protected --branch \"ODP-main\"
    # Check a specific repository for old branches inactive for 60 days matching a specific pattern. The pattern is optional, and if not provided, all branches will be checked.
    git_sync repo check --repository my-repo --old-branches --days-ago --branch-filter \"^ODP\"

NOTES:
    - At least one of --license, --protected, or --old-branches must be specified.
    - Use --all to apply checks to all configured repositories, or --repository option to target one repository. You cannot use both at the same time."

)]
#[allow(clippy::struct_excessive_bools)]
pub struct CheckRepoCommand {
    /// The repository to check
    #[arg(short, long)]
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
    /// Enable checking for old branches in target repository
    #[arg(short, long, default_value_t = false)]
    pub old_branches: bool,
    /// Number of days of inactivity after which a branch is flagged
    #[arg(short, long, default_value_t = 30)]
    pub days_ago: i64,
    /// Regex that can be used to filter branches
    #[arg(short, long)]
    pub branch_filter: Option<String>,
}

// --- PR Commands ---

/// Open a PR to merge `head` into `branch`
#[derive(Args, Clone, Debug)]
#[command(
    group(
        ArgGroup::new("target")
        .required(true)
        .args(&["all", "repository"])
    ),
    group(
        ArgGroup::new("all_sha")
        .required(false)
        .args(&["all", "sha"])
    ),
    long_about = "Create a Pull Request and optionally try to merge it automatically. You can target a single repository, or all configured repositories",
    after_help="\
EXAMPLES:
    # Open a pull request for a single repository
    git_sync pr open --repository https://github.com/my-org/my-repo --head my_feature_branch --base my_base_branch
    # Attempt to merge automatically
    git_sync pr open -r https://github.com/my-org/my-repo --head my_feature_branch --base my_base_branch --merge
    # For all repositories
    git_sync pr open --all --head my_feature_branch --base my_base_branch --merge

NOTES:
    Not all Pull Requests can be merged automatically. If there are merge conflicts,\
    the PR will still be created, but you will need to fix the conflicts.

    You can specify --sha for single repositories, but cannot use this with --all.
    If you do not specify it, it will be fetched automatically.
"
)]
pub struct CreatePRCommand {
    /// Repository for which you are creating a PR
    #[arg(short, long)]
    pub repository: Option<String>,
    /// Create a PR for all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
    /// The name of the branch where your changes are implemented
    #[arg(long)]
    pub head: String,
    /// The name of the branch you want the changes merged into
    #[arg(short, long)]
    pub base: String,
    /// The title for the PR
    #[arg(short, long)]
    pub title: String,
    /// The body for the PR
    #[arg(long)]
    pub body: Option<String>,
    /// Attempt to merge the PR after creating it.
    #[arg(long, default_value_t = false)]
    pub merge: bool,
    /// Title for the automatic commit message
    #[arg(long)]
    pub merge_title: Option<String>,
    /// Extra detail to append to automatic commit message
    #[arg(long)]
    pub merge_body: Option<String>,
    /// The method to use when merging the PR. The default, per GitHub, is "merge"
    #[arg(long, value_enum, default_value = "merge")]
    pub merge_method: MergeMethod,
    /// SHA that the pull request head must match to permit merging
    #[arg(long)]
    pub sha: Option<String>,
    /// A list of reviewers to request a review from
    #[arg(long)]
    pub reviewers: Option<Vec<String>>,
    /// Optionally delete the branch after a successful merge. Only valid if --merge is specified
    #[arg(long, requires = "merge")]
    pub delete: Option<String>,
}

/// Close a PR for a repository. Currently, doesn't do anything, since it's kind of pointless
/// to close PRs specified by number from the cli. May be updated later to be useful
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
    #[arg(short, long)]
    pub repository: Option<String>,
    /// Close a PR for all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
    /// The PR number
    #[arg(short, long)]
    pub id: u64,
    /// The base branch of the PR to close
    #[arg(short, long)]
    pub base_branch: String,
}

/// Merge a PR. Not used since if you already know the PR number, then you're probably in the web
/// ui anyway. Might later be used for something useful
#[derive(Args, Clone, Debug)]
#[command(
    group(
        ArgGroup::new("target")
        .required(true)
        .args(&["all", "repository"])
    ),
    group(
        ArgGroup::new("all_sha")
        .required(false)
        .args(&["all", "sha"])
    )
)]
pub struct MergePRCommand {
    /// Repository for which you are merging a PR
    #[arg(short, long)]
    pub repository: Option<String>,
    /// The PR number
    #[arg(short, long, name = "pull_number")]
    pub id: u64,
    /// The base branch of the PR to close
    #[arg(short, long)]
    pub base_branch: String,
    /// The method to use when merging the PR
    #[arg(short, long, value_parser = ["merge", "squash", "rebase"], default_value = "squash")]
    pub merge_method: String,
    /// Title for the automatic commit message
    #[arg(long)]
    pub commit_title: Option<String>,
    /// Extra detail to append to the automatic commit message
    #[arg(long)]
    pub commit_message: Option<String>,
    /// SHA that the pull request head must match to permit merging
    #[arg(long)]
    pub sha: Option<String>,
}

// --- Branch Commands ---

/// Delete a branch from a repository
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
    #[arg(short, long)]
    pub repository: Option<String>,
    /// Branch to delete
    #[arg(short, long)]
    pub branch: String,
    /// Delete the specified branch for all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
}

/// Change the version of the contents of a branch
#[derive(Args, Clone, Debug)]
#[command(
    group(
        ArgGroup::new("target")
        .required(true)
        .args(&["all", "repository"])
    ),
    long_about = "
Examples:
    # Change the version from 3.3.6.2-1 to 3.3.6.2-101
    git_sync branch change --all --branch ODP-3.3.6.2-1 --old 3.3.6.2-1 --new 3.3.6.2-101
Notes:
    This action is a regex match and replace, so be careful what you pass as the old version.
    Strictly speaking, this can be used to replace any matching regex within a repository

    If you run this against odp-bigtop in order to update its version, make sure that `--not-version` is *not* specified.
    This ensures that it will correctly update filenames and other odp-bigtop specific changes.
"
)]
pub struct ChangeBranchTextCommand {
    /// Change text of a branch in a repository
    #[arg(short, long)]
    pub repository: Option<String>,
    /// Branch to change text in
    #[arg(short, long)]
    pub branch: String,
    /// The old text you wish to change
    #[arg(short, long)]
    pub old: String,
    /// The new text you want to change it to
    #[arg(short, long)]
    pub new: String,
    /// Specify that the text you're modifying is a version. This affects the default commit
    /// message as well as running additional steps for odp-bigtop.
    #[arg(long, default_value_t = false)]
    pub not_version: bool,
    /// Change the text for the specified branch across all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
    /// An optional commit message to override the automatic commit message
    #[arg(short, long)]
    pub message: Option<String>,
}

/// Create a new branch.
#[derive(Args, Clone, Debug)]
#[command(
    group(
        ArgGroup::new("target")
        .required(true)
        .args(&["all", "repository"])
    ),
    group(
        ArgGroup::new("branch_tag")
        .required(true)
        .args(&["base_branch", "base_tag"])
    )
)]
pub struct CreateBranchCommand {
    /// Create a branch in this repository
    #[arg(short, long)]
    pub repository: Option<String>,
    /// New branch to create
    #[arg(short, long)]
    pub new_branch: String,
    /// The base branch for the new branch. Not valid if --base-tag is passed
    #[arg(short = 'b', long)]
    pub base_branch: Option<String>,
    /// The base tag for the new branch. Not valid if --base-branch is passed
    #[arg(short = 't', long)]
    pub base_tag: Option<String>,
    /// Create the branch for all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
}
///Show branches. Optional regex filter
#[derive(Args, Clone, Debug)]
#[command(group(
    ArgGroup::new("target")
    .required(true)
    .args(&["all", "repository"])
))]
pub struct ShowBranchCommand {
    /// The repository to show Branches for. Not valid if '--all' is set
    #[arg(short, long)]
    pub repository: Option<String>,
    /// The regex filter to apply to branch names
    #[arg(short = 'l', long, default_value = "")]
    pub filter: String,
    /// Show branches for all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
}

// --- Release Commands ---

/// Create a release for a repository.
#[derive(Args, Clone, Debug)]
#[command(
    group(
        ArgGroup::new("target")
        .required(true)
        .args(&["all", "repository"])
    ),
)]
pub struct CreateReleaseCommand {
    /// The tag to base the release off of
    #[arg(short, long)]
    pub current_release: String,
    /// The previous release tag. This is used to generate the changelog
    #[arg(short, long, required_unless_present = "skip_missing_tag")]
    pub previous_release: Option<String>,
    /// The repository for which to create the release
    #[arg(short, long)]
    pub repository: Option<String>,
    /// The name of the release. If not specified, the tag name will be used
    #[arg(long)]
    pub release_name: Option<String>,
    /// Set this release to the latest.
    #[arg(short, long, default_value_t = MakeLatest::True)]
    pub latest: MakeLatest,
    /// Create this release for all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
    /// Continue creating the release even if the `previous` tag does not exist.
    /// This will mean no release notes are generated.
    #[arg(long, default_value_t = false)]
    pub skip_missing_tag: bool,
}

// --- Backup Commands ---

/// Backup repositories
#[derive(Args, Clone, Debug)]
#[command(
    group(
        ArgGroup::new("target")
        .required(true)
        .args(&["all", "repository"])
    ),
)]
#[allow(clippy::struct_excessive_bools)]
pub struct BackupRepoCommand {
    /// The repository to back up
    #[arg(short, long)]
    pub repository: Option<String>,
    /// Backup all configured repositories
    #[arg(short, long, default_value_t = false)]
    pub all: bool,
    /// The directory to store the backups in. If not specified, the current directory will be used
    #[arg(short, long, value_parser = dir_exists)]
    pub path: Option<PathBuf>,
    /// The destination for the backup
    #[arg(short, long, default_value_t = BackupDestination::Local)]
    pub destination: BackupDestination,
    /// Disable cloning atomically. If something goes wrong during the clone, your backup may be in
    /// an undefined state.
    #[arg(long = "no-atomic", default_value_t = true, action = clap::ArgAction::SetFalse)]
    pub atomic: bool,
    /// Update an existing backup instead of making a new mirror clone. Does not do anything
    /// currently.
    #[arg(short, long, default_value_t = false)]
    pub update: bool,
    /// Enable the backup blacklist to prevent repositories from being backed up from the
    /// `backup_blacklist` set in git-manage.toml. This option will be ignored if
    /// `--repository` is specified
    #[arg(long, default_value_t = false)]
    pub blacklist: bool,
    /// Bucket name for where you want to store your backup if using `S3`
    #[cfg(feature = "aws")]
    #[arg(short, long, required_if_eq("destination", "s3"))]
    pub bucket: Option<String>,
}

/// Check that the directory passed is a valid and existing directory. If it isn't, try to create
/// it.
fn dir_exists(s: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(s);
    if p.is_dir() {
        return Ok(p);
    }
    if let Ok(m) = std::fs::metadata(&p) {
        let metadata = m.file_type();
        let file_type = if metadata.is_file() {
            "file"
        } else if metadata.is_symlink() {
            "symlink"
        } else {
            "unknown"
        };
        Err(format!("Path '{s}' exists but it is a {file_type}"))
    } else {
        if let Err(e) = std::fs::create_dir_all(&p) {
            return Err(format!("Could not create '{s}': {e}"));
        }
        Ok(p)
    }
}

#[derive(Args, Clone, Debug)]
pub struct PruneBackupCommand {
    /// Path to the backup directory. This is only implemented for local backups
    #[arg(short, long, value_parser = dir_exists)]
    pub directory: PathBuf,
}

// === Subcommand Enums ===

/// Define all the valid commands for acting on pull requests
#[derive(Subcommand, Clone, Debug)]
pub enum PRCommand {
    /// Create a new PR
    Open(CreatePRCommand),
}

/// Define all the valid commands for acting on backups
#[derive(Subcommand, Clone, Debug)]
pub enum BackupCommand {
    /// Create new backups. This operation can take a long time depending on the size of the repo.
    Create(BackupRepoCommand),
    /// Clean up old backups
    Clean(PruneBackupCommand),
}

/// Define all the valid commands for action on releases
#[derive(Subcommand, Clone, Debug)]
pub enum ReleaseCommand {
    /// Create a new release
    Create(CreateReleaseCommand),
    // Delete(DeleteReleaseCommand)
}

/// Define all the valid commands for acting on Tags
#[derive(Subcommand, Clone, Debug)]
pub enum TagCommand {
    /// Compare tags between repositories
    Compare(CompareTagCommand),
    /// Create a new tag
    Create(CreateTagCommand),
    /// Delete a tag
    Delete(DeleteTagCommand),
    /// Sync tags
    Sync(SyncTagCommand),
    ///Show Tags for a repository
    Show(ShowTagCommand),
}

/// Define all the valid commands for acting on repositories
#[derive(Subcommand, Clone, Debug)]
pub enum RepoCommand {
    /// Sync repositories
    Sync(SyncRepoCommand),
    /// Check repositories for various conditions
    Check(CheckRepoCommand),
}

/// Define all the valid commands for acting on branches
#[derive(Subcommand, Clone, Debug)]
pub enum BranchCommand {
    /// Create a branch for repositories
    Create(CreateBranchCommand),
    /// Delete a branch in repositories
    Delete(DeleteBranchCommand),
    /// Modify text in a branch for repositories
    Modify(ChangeBranchTextCommand),
    /// Show branches for repositories
    Show(ShowBranchCommand),
}

/// The top-level command enum for the CLI
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage branches
    #[command(arg_required_else_help = true)]
    Branch {
        #[command(subcommand)]
        cmd: BranchCommand,
    },
    /// Generate a default config
    #[command()]
    Config {
        /// Path to save the config file. If not specified, it will default to
        /// $XDG_CONFIG_HOME/git-manage.toml or $HOME/.config/git-manage.toml
        #[arg(short, long)]
        file: Option<PathBuf>,
        /// Overwrite existing config file if it exists
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Manage repositories
    #[command(arg_required_else_help = true)]
    Repo {
        #[command(subcommand)]
        cmd: RepoCommand,
    },
    /// Manage tags
    #[command(arg_required_else_help = true)]
    Tag {
        #[command(subcommand)]
        cmd: TagCommand,
    },
    /// Manage releases
    #[command(arg_required_else_help = true)]
    Release {
        #[command(subcommand)]
        cmd: ReleaseCommand,
    },
    /// Manage backups of repositories
    #[command(arg_required_else_help = true)]
    Backup {
        #[command(subcommand)]
        cmd: BackupCommand,
    },
    /// Manage Pull Requests
    #[command(arg_required_else_help = true)]
    PR {
        #[command(subcommand)]
        cmd: PRCommand,
    },
    /// Generate shell completions or man pages.
    #[command(hide = true)]
    Generate {
        /// What to generate. Can be shell completion for bash, zsh, fish, or man pages.
        #[arg(long, value_parser = ["bash", "zsh", "fish", "man"])]
        kind: String,
        /// An optional output path. If not specified, the current directory will be used instead
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

// === CLI Entrypoints ===

/// Parse the command line arguments and validate them.
pub fn parse_args() -> AppArgs {
    let app = AppArgs::try_parse();
    match app {
        Ok(app) => {
            if let Command::Repo {
                cmd: RepoCommand::Sync(sync_cmd),
            } = &app.command
                && sync_cmd.recursive
                && sync_cmd.all
                && !sync_cmd.force
            {
                eprintln!(
                    "Error: when syncing repositories, --force is required when --all and --recursive are set."
                );
                std::process::exit(1);
            }
            if !(app.repository_type == RepositoryType::All
                || app.repository_type == RepositoryType::Fork)
                && app.with_fork_workaround
            {
                eprintln!(
                    "Error: --with-fork-workaround is only valid when --repository-type is 'all' or 'fork'"
                );
                std::process::exit(2);
            }
            app
        }
        Err(err) => {
            err.print().unwrap();
            std::process::exit(1);
        }
    }
}

pub fn cli() -> clap::Command {
    AppArgs::command()
}
