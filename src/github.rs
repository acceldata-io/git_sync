use std::collections::HashSet;

use futures::StreamExt;
use futures::future::try_join;
use futures::stream::FuturesUnordered;
use octocrab::Octocrab;

use crate::error::GitError;
use crate::utils::{get_repo_info_from_url, RepoInfo};

/// Contains information about tags for a forked repo, its parent,
/// and the tags that are missing from the fork
pub struct ComparisonResult {
    /// All tags found in the forked repository
    pub fork_tags: Vec<String>,
    /// All tags found in the parent repository
    pub parent_tags: Vec<String>,
    /// All tags missing in the forked repository
    pub missing_in_fork: Vec<String>,
}

/// Get the parent repository for a forked github repository.
pub async fn get_parent_repo(
    octocrab: &Octocrab,
    url: &str,
) -> Result<RepoInfo, GitError> {
    let info = get_repo_info_from_url(url)?;
    let (owner, repo) = (info.owner, info.repo_name);
    let repo_info = octocrab.repos(owner, repo).get().await?;

    let parent = repo_info.parent.ok_or(GitError::NotAFork)?;

    let parent_owner = parent.owner.ok_or(GitError::MissingParentOwner)?.login;

    let url = parent.html_url.ok_or_else(|| GitError::InvalidRepository(url.to_string()))?.to_string();

    Ok(RepoInfo {
        owner: parent_owner,
        repo_name: parent.name,
        main_branch: parent.default_branch,
        url,
    })
}
/// Get all tags for a repository
async fn get_tags(
    octocrab: &Octocrab,
    url: &str,
) -> Result<Vec<String>, GitError> {
    let mut page: u32 = 1;
    let mut all_tags: Vec<String> = Vec::new();
    let per_page = 100;
    let info = get_repo_info_from_url(url)?;
    let (owner, repo) = (info.owner, info.repo_name);
    loop {
        let tags = octocrab
            .repos(&owner, &repo)
            .list_tags()
            .per_page(per_page)
            .page(page)
            .send()
            .await?;

        let items = tags.items;
        if items.is_empty() {
            break;
        }
        for tag in items {
            all_tags.push(tag.name);
        }
        page += 1;
    }
    Ok(all_tags)
}

/// Compare tags against a repository and its parent
async fn compare_tags(
    octocrab: &Octocrab,
    fork_url: &str,
    parent_url: &str,
) -> Result<ComparisonResult, GitError> {
    let (fork_tags, parent_tags) = try_join(
        get_tags(octocrab, fork_url),
        get_tags(octocrab, parent_url),
    )
    .await?;

    let fork_tags_set: HashSet<_> = fork_tags.iter().collect();
    let parent_tags_set: HashSet<_> = parent_tags.iter().collect();

    let missing_in_fork: Vec<_> = parent_tags_set
        .difference(&fork_tags_set)
        .map(|&s| s.to_string())
        .collect();

    Ok(ComparisonResult {
        fork_tags,
        parent_tags,
        missing_in_fork,
    })
}
/// Get a diff of tags between a single forked repository and its parent repository.
pub async fn diff_tags(octocrab: &Octocrab, url: &str) -> Result<(), GitError> {
    println!("Fetching parent repo info for {url}");
    let info = get_repo_info_from_url(url)?;
    let (owner, repo) = (info.owner, info.repo_name);
    let parent = get_parent_repo(octocrab, url).await?;

    println!(
        "Comparing tags in {owner}/{repo} and {}/{}",
        parent.owner, parent.repo_name
    );
    let mut comparison = compare_tags(octocrab, url, &info.url).await?;
    println!(
        "Fork has {} tags, parent has {} tags",
        comparison.fork_tags.len(),
        comparison.parent_tags.len()
    );

    if !comparison.missing_in_fork.is_empty() {
        comparison.missing_in_fork.sort();
        println!(
            "\nTags missing in fork: {}",
            comparison.missing_in_fork.len()
        );
        for tag in &comparison.missing_in_fork {
            println!(" - {tag}");
        }
    } else {
        println!("{repo} up to date");
    }

    Ok(())
}

/// Get a diff of all configured repositories tags, compared against their parent.
pub async fn diff_tags_all(
    octocrab: &Octocrab,
    repos: Vec<String>,
) -> Result<(), GitError> {
    #[derive(Debug)]
    struct ParseRepo {
        owner: String,
        repo_name: String,
    }
    let mut futures = FuturesUnordered::new();
    let repositories: Vec<Result<RepoInfo,_>> = repos.iter().map(|url| get_repo_info_from_url(url)).collect();
    for repo in repositories.into_iter().flatten() {
        let octo = octocrab.clone();
        let url = repo.url.clone();
        let owner = repo.owner.clone();
        let repo = repo.repo_name.clone();
        futures.push(async move {
            let result = diff_tags(&octo, &url).await;
            (owner, repo, result)
        });

    }
    while let Some((owner, repo, result)) = futures.next().await {
        match result {
            Ok(()) => println!("OK: {owner}/{repo}"),
            Err(e) => eprintln!("FAILED: {owner}/{repo}: {e}"),
        }
    }
    Ok(())
}

/// Sync a single repository with its parent repository.
pub async fn sync_fork(octocrab: &Octocrab, url: &str) -> Result<(), GitError> {
    let info = get_repo_info_from_url(url)?;
    let (owner, repo) = (info.owner, info.repo_name);
    println!("Syncing {owner}/{repo} with its parent repository...");

    let parent = get_parent_repo(octocrab, url).await?;
    let body = serde_json::json!({"branch": parent.main_branch});
    let response: Result<serde_json::Value, octocrab::Error> = octocrab
        .post(
        format!("/repos/{owner}/{repo}/merge-upstream"),
        Some(&body),
    ).await;

    match response{
        Ok(_) => {
            println!("Successfully synced {owner}/{repo} with its parent repository");
        },
        Err(err) => {
            let (status, message) = get_http_status(&err);
            match (status, message) {
                (Some(code), Some(message)) => eprintln!("Failed to sync {owner}/{repo}: HTTP {code:?} - {message:?}"),
                (Some(code), None) => eprintln!("Failed to sync {owner}/{repo}: HTTP {code:?}"),
                _ => eprintln!("Failed to sync {owner}/{repo}: {err}"),
            }
            return Err(GitError::GithubApiError(err));
        }
    }
    Ok(())
}

/// Sync all configured repositories. Only repositories that have a parent repository
/// should be passed to this function
pub async fn sync_all_forks(
    octocrab: &Octocrab,
    repos: Vec<String>,
) -> Result<(), GitError> {
    let mut futures = FuturesUnordered::new();
    for repo in repos {
        let octo = octocrab.clone();
        futures.push(async move {
            let result = sync_fork(&octo, &repo).await;
            (repo, result)
        });
    }

    while let Some((repo, result)) = futures.next().await {
        match result {
            Ok(_) => println!("Successfully synced {repo}"),
            Err(e) => eprintln!("Sync failed for {repo}: {e}"),
        }
    }
    Ok(())
}

pub async fn get_branch_protection(octocrab: &Octocrab, url: &str, branch: &str) -> Result<bool, GitError> {
    let info = get_repo_info_from_url(url)?;
    let get_url = format!(
        "/repos/{}/{}/branches/{branch}/protection",
        info.owner, info.repo_name
    );
    let response:Result<serde_json::Value, octocrab::Error> = octocrab.get(
        get_url,
        None::<&()>,
    ).await;
    match response{
        Ok(_) =>Ok(true),
        Err(err) => {
            let (status, message) = get_http_status(&err);
            match (status, message) {
                (Some(code), Some(message)) => eprintln!("Failed to get protection rules for {url}: HTTP {code:?} - {message:?}"),
                (Some(code), None) => eprintln!("Failed get protection rules for {url}: HTTP {code:?}"),
                _ => eprintln!("Failed to get protection rules for {url}: {err}"),
            }
            Err(GitError::GithubApiError(err))
        }
    }
}

/// Convert github http status errors to a usable string message
fn get_http_status(err: &octocrab::Error) -> (Option<http::StatusCode>, Option<String>) {
    if let octocrab::Error::GitHub { source, .. } = err {
        let status = source.status_code;
        let message = source.message.clone();
        return (Some(status), Some(message));
    }
    (None, None)
}
