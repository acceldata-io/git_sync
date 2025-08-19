use std::collections::HashSet;

use futures::StreamExt;
use futures::FutureExt;
use futures::future::try_join;
use futures::stream::FuturesUnordered;
use octocrab::Octocrab;

use crate::error::GitError;
use crate::utils::repo::CheckResult;
use crate::utils::repo::{get_repo_info_from_url,RepoInfo, RepoChecks};

use crate::handle_api_response;

/// Contains information about tags for a forked repo, its parent,
/// and the tags that are missing from the fork
struct ComparisonResult {
    /// All tags found in the forked repository
    pub fork_tags: Vec<String>,
    /// All tags found in the parent repository
    pub parent_tags: Vec<String>,
    /// All tags missing in the forked repository
    pub missing_in_fork: Vec<String>,
}

/// Github api entry point
pub struct GithubClient {
    /// Octocrab client. This can be trivially cloned
    octocrab: Octocrab,
}

impl GithubClient {
    pub fn new(github_token: String) -> Result<Self, GitError> {
        let octocrab = Octocrab::builder()
            .personal_token(github_token)
            .build()
            .map_err(GitError::GithubApiError)?;

        Ok(Self {
            octocrab
        })
    }
    /// Get the parent repository of a github repository.
    pub async fn get_parent_repo(
        &self,
        url: &str,
    ) -> Result<RepoInfo, GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let repo_info = self.octocrab.clone().repos(owner, repo).get().await?;

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
    pub async fn get_tags(
        &self,
        url: &str,
    ) -> Result<Vec<String>, GitError> {
        let mut page: u32 = 1;
        let mut all_tags: Vec<String> = Vec::new();
        let per_page = 100;
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        loop {
            let tags = self.octocrab.clone()
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
    pub async fn compare_tags(
        &self,
        fork_url: &str,
        parent_url: &str,
    ) -> Result<ComparisonResult, GitError> {
        let (fork_tags, parent_tags) = try_join(
            self.get_tags( fork_url),
            self.get_tags(parent_url),
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
    pub async fn diff_tags(&self, url: &str) -> Result<(), GitError> {
        println!("Fetching parent repo info for {url}");
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let parent = self.get_parent_repo( url).await?;

        println!(
            "Comparing tags in {owner}/{repo} and {}/{}",
            parent.owner, parent.repo_name
        );
        let mut comparison = self.compare_tags(url, &info.url).await?;
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
    pub async fn diff_all_tags(
        &self,
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
            let url = repo.url.clone();
            let owner = repo.owner.clone();
            let repo = repo.repo_name.clone();
            futures.push(async move {
                let result = self.diff_tags(&url).await;
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
    pub async fn sync_fork(&self, url: &str) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        println!("Syncing {owner}/{repo} with its parent repository...");
        
        let parent = self.get_parent_repo(url).await?;
        let body = serde_json::json!({"branch": parent.main_branch});
        let response: Result<serde_json::Value, octocrab::Error> = self.octocrab.clone() 
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
        &self,
        repos: Vec<String>,
    ) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repos {
            futures.push(async move {
                let result = self.sync_fork( &repo).await;
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
    /// Get the most recent commit of a branch, so we can use that to create and delete it
    async fn get_branch_sha(&self, url: &str, branch: &str) -> Result<String, GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let response: Result<serde_json::Value, octocrab::Error> = self.octocrab.clone() 
            .get(
            format!("/repos/{owner}/{repo}/branches/{branch}"),
            None::<&()>,
        ).await;
        
        let sha: String = handle_api_response!(
            response,
            format!("{owner}/{repo}:{branch}"),
            |body: serde_json::Value|{
                let sha = body.get("commit")
                    .and_then(|c| c.get("sha"))
                    .and_then(|s| s.as_str())
                    .ok_or(GitError::NoSuchBranch(branch.to_string()))?;
                Ok::<String, GitError>(sha.to_string())
            },
        )?;
        Ok(sha)
    }
    /// Create a branch from some base branch in a repository
    pub async fn create_branch(&self, url: &str, base_branch: &str, new_branch: &str) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let sha = self.get_branch_sha(url, base_branch).await?;

        let response: Result<serde_json::Value, octocrab::Error> = self.octocrab.clone()
            .post(
                format!("/repos/{owner}/{repo}/git/refs"),
                Some(&serde_json::json!({
                    "ref": format!("refs/heads/{new_branch}"),
                    "sha": sha,
                })),
            )
            .await;
        handle_api_response!(
            response,
            format!("Can't create {owner}/{repo}:{new_branch}"),
            |_| {
                Ok(())
            },
        )
    }
    /// Create all passed branches for each repository provided
    pub async fn create_all_branches(&self, base_branch: &str, new_branch:&str, repositories: &Vec<String>) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            futures.push(async move {
                let result = self.create_branch( repo, base_branch, new_branch).await;
                (repo, result)
            });
        }

        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(_) => println!("Successfully created {new_branch} for {repo}"),
                Err(e) => eprintln!("Failed to create {new_branch} for {repo}: {e}"),
            }
        }
        Ok(())

    }
    /// Delete a branch from a repository
    pub async fn delete_branch(&self, url: &str, branch: &str) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let response = self.octocrab.clone()
            ._delete(
                format!("/repos/{owner}/{repo}/git/refs/heads/{branch}"),
                None::<&()>,
            )
            .await;
        match response {
            Ok(resp) => {
                if resp.status().is_success() {
                    println!("Successfully deleted branch '{branch}' for {repo}");
                    Ok(())
                } else {
                    let status_code = resp.status().as_u16();

                    match status_code {
                        422 => {
                            println!("Branch '{branch}' does not exist in {repo}. Nothing to delete.");
                            Ok(())
                        }
                        _ =>  Err(GitError::Other(format!("Cannot delete {branch}: {}", resp.status())))

                    }
                }
            },
            Err(e) => Err(GitError::Other(format!("Cannot delete {branch}: {e}"))),
        }

    }

    pub async fn delete_all_branches(&self, branch: &str, repositories: &Vec<String>) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            futures.push(async move {
                let result = self.delete_branch(repo, branch).await;
                (repo, result)
            });
        }
        
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(_) => println!("Successfully deleted {branch} in {repo}"),
                Err(e) => eprintln!("Failed to delete {branch} for {repo}: {e}")
            }
        }
        Ok(())
    }

    pub async fn create_tag(&self, url: &str, tag: &str, branch: &str) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let sha = self.get_branch_sha(url, branch).await?;
        let (owner, repo) = (info.owner, info.repo_name);

        let response:Result<serde_json::Value, octocrab::Error> = self.octocrab.clone()
            .post(
                format!("/repos/{owner}/{repo}/git/refs"),
                Some(&serde_json::json!({
                    "ref": format!("refs/tags/{tag}"), 
                    "sha": sha,
                })),
            ).await;
        handle_api_response!(
            response,
            format!("Unable to create '{tag}' for {repo}"),
            |_| {
                Ok(())
            },
        )
    }
    
    pub async fn create_all_tags(&self, tag: &str, branch: &str, repositories: Vec<String>) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            futures.push(async move {
                let result = self.create_tag(&repo, tag, branch).await;
                (repo, result)
            });
        }

        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(_) => println!("Successfully created tag '{tag}' for '{repo}'"),
                Err(e) => eprintln!("Failed to create tag '{tag}' for '{repo}': {e}"),
            }
        }
        Ok(())
    }
    /// Delete the specified tag for a repository
    pub async fn delete_tag(&self, url: &str, tag: &str) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let response = self.octocrab.clone()
            ._delete(
                format!("/repos/{owner}/{repo}/git/refs/tags/{tag}"),
                None::<&()>,
            ).await;
        
        match response {
            Ok(resp) => {
                if resp.status().is_success() {
                    println!("Successfully deleted tag '{tag}' for {repo}");
                    Ok(())
                } else {
                    let status_code = resp.status().as_u16();

                    match status_code {
                        422 => {
                            println!("Tag '{tag}' does not exist in {repo}. Nothing to delete.");
                            Ok(())
                        }
                        _ =>  Err(GitError::Other(format!("Cannot delete {tag}: {}", resp.status())))

                    }
                }
            },
            Err(e) => Err(GitError::Other(format!("Cannot delete {tag}: {e}"))),
        }

    }
    /// Delete the specified tag for all configured repositories
    pub async fn delete_all_tags(&self, tag: &str, repositories: &Vec<String>) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            futures.push(async move {
                let result = self.delete_tag(repo, tag).await;
                (repo, result)
            });
        }

        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(_) => println!("Successfully deleted tag '{tag}' for {repo}"),
                Err(e) => eprintln!("Failed to delete tag '{tag}' for {repo}: {e}"),
            }
        }
        Ok(())
    }

    async fn get_branch_protection(&self, url: &str, branch: &str) -> Result<CheckResult, GitError> {
        let info = get_repo_info_from_url(url)?;
        let get_url = format!(
            "/repos/{}/{}/branches/{branch}/protection",
            info.owner, info.repo_name
        );
        let response:Result<serde_json::Value, octocrab::Error> = self.octocrab.clone().get(
            get_url,
            None::<&()>,
        ).await;
        handle_api_response!(
            response,
            format!("Unable to get branch protection for {}/{}:{}", info.owner, info.repo_name, branch),
            |body: serde_json::Value| {
                let enabled:bool = body.get("enabled")
                    .and_then(|e| e.as_bool())
                    .unwrap_or(false);
                Ok(CheckResult::Protected(enabled))
            },
        )
    }

    pub async fn check_repository(&self, url: &str, checks: &RepoChecks) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let (protected, license) = (checks.protected, checks.license);

        println!("Checking repository {owner}/{repo}");
        let mut futures = FuturesUnordered::new();
        if protected {
            let url = info.url.clone();
            let branch = info.main_branch.clone().unwrap_or_else(|| "main".to_string());
            futures.push(async move {
                self.get_branch_protection(&url, &branch).await
            }.boxed());
        }

        if license {
            let url = info.url.clone();
            futures.push(
                async move {
                    self.get_license(&url).await
            }.boxed());
        }

        let mut errors = Vec::new();
        while let Some(result) = futures.next().await {
            match result {
                Ok(CheckResult::License(license)) => println!("License for {owner}/{repo} is '{license}'"),
                Ok(CheckResult::Protected(true)) => println!("Main branch for {owner}/{repo} is protected"),
                Ok(CheckResult::Protected(false)) => {
                    eprintln!("Main branch for {owner}/{repo} is not protected");
                    errors.push(GitError::NoMainBranchProtection(format!("{owner}/{repo}")));
                },
                Err(e) => {
                    eprintln!("Check failed for {owner}/{repo}: {e}");
                    errors.push(e);
                },
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(())
    }
    pub async fn check_all_repositories(&self, repositories: Vec<String>, checks: &RepoChecks) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            futures.push(async move {
                self.check_repository(&repo, checks).await
            });
        }

        let mut errors = Vec::new();
        while let Some(result) = futures.next().await {
            match result {
                Ok(_) => {},
                Err(e) => {
                    errors.push(e);
                },
            }
        }
        if !errors.is_empty() {
            Err(GitError::MultipleErrors(errors))
        } else {
            Ok(())
        }
    }

    /// Get a license for a repository
    async fn get_license(&self, url:&str) -> Result<CheckResult, GitError> {
        let info = get_repo_info_from_url(url)?;
        let content = self.octocrab.clone()
            .repos(&info.owner, &info.repo_name)
            .license()
            .await?;
        if let Some(license) = content.license {
            Ok(CheckResult::License(license.name))
        }else {
            Err(GitError::MissingLicense(url.to_string()))
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
