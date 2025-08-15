use std::collections::HashSet;

use futures::StreamExt;
use futures::future::try_join;
use futures::stream::FuturesUnordered;
use octocrab::Octocrab;

use crate::error::GitError;

pub struct RepoInfo {
    pub owner: String,
    pub name: String,
}

pub struct ComparisonResult {
    pub fork_tags: Vec<String>,
    pub parent_tags: Vec<String>,
    pub missing_in_fork: Vec<String>,
}

pub async fn get_parent_repo(
    octocrab: &Octocrab,
    owner: &str,
    repo: &str,
) -> Result<RepoInfo, GitError> {
    let repo_info = octocrab.repos(owner, repo).get().await?;

    let parent = repo_info.parent.ok_or(GitError::NotAFork)?;

    let parent_owner = parent.owner.ok_or(GitError::MissingParentOwner)?.login;

    Ok(RepoInfo {
        owner: parent_owner,
        name: parent.name,
    })
}

pub async fn get_all_tags(
    octocrab: &Octocrab,
    owner: &str,
    repo: &str,
) -> Result<Vec<String>, GitError> {
    let mut page: u32 = 1;
    let mut all_tags: Vec<String> = Vec::new();
    let per_page = 100;

    loop {
        let tags = octocrab
            .repos(owner, repo)
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

pub async fn compare_tags(
    octocrab: &Octocrab,
    fork_owner: &str,
    fork_repo: &str,
    parent_owner: &str,
    parent_repo: &str,
) -> Result<ComparisonResult, GitError> {
    let (fork_tags, parent_tags) = try_join(
        get_all_tags(octocrab, fork_owner, fork_repo),
        get_all_tags(octocrab, parent_owner, parent_repo),
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

pub async fn diff_tags(octocrab: &Octocrab, owner: &str, repo: &str) -> Result<(), GitError> {
    println!("Fetching parent repo info for {repo}");
    let parent = get_parent_repo(octocrab, owner, repo).await?;

    println!(
        "Comparing tags in {owner}/{repo} and {}/{}",
        parent.owner, parent.name
    );
    let mut comparison = compare_tags(octocrab, owner, repo, &parent.owner, &parent.name).await?;
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

pub async fn diff_tags_all(
    octocrab: &Octocrab,
    repos: Vec<(String, String)>,
) -> Result<(), GitError> {
    let mut futures = FuturesUnordered::new();
    for (owner, repo) in repos {
        let octo = octocrab.clone();
        let owner = owner.clone();
        let repo = repo.clone();
        futures.push(async move {
            let result = diff_tags(&octo, &owner, &repo).await;
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
