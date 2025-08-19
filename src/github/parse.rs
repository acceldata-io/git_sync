use crate::cli::*;
use crate::GitError;
use crate::github::client::GithubClient;
use crate::init::generate_config;
use crate::utils::repo::RepoChecks;

/// Parse the argument that gets passed, and run their associated methods
pub async fn match_arguments(command: &Command, token: &str, repositories: Option<Vec<String>>) -> Result<(), GitError> {
    let client = GithubClient::new(token.to_string())?;
    let repos: Vec<String> = repositories.unwrap_or_default();
    match &command {
        Command::Tag { cmd, repository} => match cmd {
            TagCommand::Compare(compare_cmd)=> {
                compare_cmd.validate().map_err(GitError::Other)?;
 
                if compare_cmd.all && !repos.is_empty() {
                    client.diff_all_tags(repos).await?

                } else if compare_cmd.all {
                    return Err(GitError::NoReposConfigured);
                } else if let Some(repository) = repository{
                    client.diff_tags(repository).await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            },
            TagCommand::Create(create_cmd) => {
                let tag = &create_cmd.tag;
                let branch = &create_cmd.branch;
                if create_cmd.all {
                    client.create_all_tags(tag, branch, repos).await?
                } else if let Some(repository) = repository {
                    client.create_tag(repository, &create_cmd.tag, &create_cmd.branch).await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            },
            TagCommand::Delete(delete_cmd) => {
                if delete_cmd.all {
                    client.delete_all_tags(&delete_cmd.tag, &repos).await?
                } else if let Some(repository) = repository {
                    client.delete_tag(repository, &delete_cmd.tag).await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            },
        },
        Command::Repo { cmd, repository } => match cmd {
            RepoCommand::Sync(sync_cmd) => {
                if sync_cmd.all {
                    client.sync_all_forks(repos).await?
                } else if let Some(repository) = repository {
                    client.sync_fork(repository).await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            },
            RepoCommand::Check(check_cmd) => {
                let (protected, license) = (check_cmd.protected, check_cmd.license);
                if check_cmd.all {
                    client.check_all_repositories(repos, &RepoChecks{protected, license}).await?
                } else if let Some(repository) = repository {
                    client.check_repository(repository, &RepoChecks{protected, license}).await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            }
        },
        Command::Branch{cmd, repository} => match cmd {
            BranchCommand::Create(create_cmd) => {
                let (base, new) = (create_cmd.base_branch.clone(), create_cmd.new_branch.clone());
                if create_cmd.all {
                    client.create_all_branches(&base, &new,&repos).await?
                } else if let Some(repository) = repository {
                    client.create_branch(repository, &base, &new ).await?
                }
            },
            BranchCommand::Delete(delete_cmd) => {
                if delete_cmd.all {
                    client.delete_all_branches(&delete_cmd.branch, &repos).await?
                } else if let Some(repository) = repository {
                    client.delete_branch(repository, &delete_cmd.branch).await?
                }
            },
        },
        Command::InitConfig { path, force } => {
            generate_config(path.clone(), *force)?;
        },

    }
    Ok(())
}
