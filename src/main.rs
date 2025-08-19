mod cli;
mod config;
mod error;
mod github;
mod init;
mod utils;

use cli::*;//{Command, SyncCommand, parse_args};
use config::Config;
use error::GitError;
use github::parse;



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

    let repos = config.get_fork_repositories();
    //let client = GithubClient::new(token)?;
    parse::match_arguments(&args.command, &token, repos).await?;
    //client.get_branch_protection("https://github.com/JeffreySmith/qmk_firmware", "main").await?;

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
