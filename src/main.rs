mod cli;
mod config;
mod error;
mod github;
mod init;
mod utils;

use cli::*;//{Command, SyncCommand, parse_args};
use config::Config;
use error::GitError;
use github::{diff_tags, diff_tags_all};
use octocrab::Octocrab;

use init::generate_config;

use crate::github::get_branch_protection;

#[tokio::main]
async fn main() -> Result<(), GitError> {
    let args = parse_args();


    let config = if let Some(config_path) = &args.file {
        Config::from_file(config_path)?
    } else {
        Config::load()
    };

    let token = args
        .token
        .or_else(|| config.get_github_token())
        .ok_or(GitError::MissingToken)?;

    let repos = config.get_fork_repositories().unwrap_or_default();

    let octocrab = Octocrab::builder()
        .personal_token(token)
        .build()
        .map_err(GitError::GithubApiError)?;
    get_branch_protection(&octocrab, "https://github.com/acceldata-io/kudu", "ODP-main").await?;
    match &args.command{
        Command::InitConfig {path, force} => {
            generate_config(path.clone(), *force)?;
        }
        _ => {}
    }
    /*match &args.command {
        Command::Compare { cmd, owner, repo } => match cmd {
            None => {
                match (owner, repo) {
                    (Some(owner), Some(repo)) => {
                        diff_tags(&octocrab, owner, repo).await
                    },
                    (Some(_), None) => Err(GitError::MissingRepoName),
                    (None, Some(repo)) => Err(GitError::MissingOwner(repo.clone())),
                    _ => Err(GitError::NoOwnerOrRepo),
                }
            },
            Some(_) => diff_tags_all(&octocrab, repos).await,
        },
        Command::Sync {cmd, repo, sync_tags} => match cmd {
            None => {
                println!("{sync_tags}");
                if let Some(repo) = repo {
                    println!("{repo:?}");
                    Ok(())

                } else {
                    return Err(GitError::MissingRepoName);
                }
            },
            Some(SyncCommand::All) => {
                if repos.is_empty() {
                    return Err(GitError::NoReposConfigured);
                }
                for (owner, repo_name) in &repos {
                    println!("Syncing {owner}/{repo_name}");
                }
                Ok(())
            },
        },
        Command::InitConfig{path, force}  => {
            generate_config(path.clone(), *force)?;
            Ok(())
        },
    }?;
*/

    Ok(())
}
