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
use chrono::{DateTime, Utc};
use futures::FutureExt;
use futures::StreamExt;
use futures::future::try_join;
use futures::stream::FuturesUnordered;
use octocrab::Octocrab;
use octocrab::models::repos::ReleaseNotes;
use octocrab::params::repos::Reference;
use regex::Regex;
use std::collections::HashMap;
use std::collections::HashSet;

use tabled::{
    Table,
    builder::Builder,
    settings::{
        Alignment, Padding, Panel,
        panel::Header,
        style::{HorizontalLine, Style},
    },
};

use crate::error::GitError;
use crate::utils::repo::CheckResult;
use crate::utils::repo::{RepoChecks, RepoInfo, get_repo_info_from_url};

use crate::handle_api_response;

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

/// Github api entry point
pub struct GithubClient {
    /// Octocrab client. This can be trivially cloned
    pub octocrab: Octocrab,
    //pub name: String,
    //pub email: String,
}

impl GithubClient {
    pub fn new(github_token: String) -> Result<Self, GitError> {
        let octocrab = Octocrab::builder()
            .personal_token(github_token)
            .build()
            .map_err(GitError::GithubApiError)?;

        Ok(Self { octocrab })
    }
    /// Get the parent repository of a github repository.
    pub async fn get_parent_repo(&self, url: &str) -> Result<RepoInfo, GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let repo_info = self.octocrab.clone().repos(owner, repo).get().await?;

        let parent = *repo_info.parent.ok_or(GitError::NotAFork)?;

        let parent_owner = parent.owner.ok_or(GitError::NoUpstreamRepo)?.login;

        let url = parent
            .html_url
            .ok_or_else(|| GitError::InvalidRepository(url.to_string()))?
            .to_string();
        Ok(RepoInfo {
            owner: parent_owner,
            repo_name: parent.name,
            url,
            main_branch: parent.default_branch,
        })
    }
    /// Get all tags for a repository
    pub async fn get_tags(&self, url: &str) -> Result<Vec<String>, GitError> {
        let mut page: u32 = 1;
        let mut all_tags: Vec<String> = Vec::new();
        let per_page = 100;
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        loop {
            let tags = self
                .octocrab
                .clone()
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
        println!("{fork_url} : {parent_url}");
        let (fork_tags, parent_tags) =
            try_join(self.get_tags(fork_url), self.get_tags(parent_url)).await?;

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
        let parent = self.get_parent_repo(url).await?;

        println!(
            "Comparing tags in {owner}/{repo} and {}/{}",
            parent.owner, parent.repo_name
        );
        let mut comparison = self.compare_tags(url, &parent.url).await?;
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
    pub async fn diff_all_tags(&self, repos: Vec<String>) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        let repositories: Vec<Result<RepoInfo, _>> = repos
            .iter()
            .map(|url| get_repo_info_from_url(url))
            .collect();
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
        let response: Result<serde_json::Value, octocrab::Error> = self
            .octocrab
            .clone()
            .post(format!("/repos/{owner}/{repo}/merge-upstream"), Some(&body))
            .await;

        handle_api_response!(
            response,
            format!("Unable to sync {owner}/{repo} with its parent repository"),
            |_| {
                println!("Successfully synced {owner}/{repo} with its parent repository");
                Ok::<(), GitError>(())
            },
        )?;
        Ok(())
    }

    /// Sync all configured repositories. Only repositories that have a parent repository
    /// should be passed to this function
    pub async fn sync_all_forks(&self, repos: Vec<String>) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repos {
            futures.push(async move {
                let result = self.sync_fork(&repo).await;
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
        let response = self
            .octocrab
            .clone()
            .repos(&owner, &repo)
            .get_ref(&Reference::Branch(branch.to_string()))
            .await?;

        let sha = match response.object {
            octocrab::models::repos::Object::Commit { sha, .. } => sha,
            _ => return Err(GitError::NoSuchBranch(branch.to_string())),
        };

        Ok(sha)
    }
    /// Get the sha of a tag
    async fn get_tag_sha(&self, url: &str, tag: &str) -> Result<String, GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let response = self
            .octocrab
            .clone()
            .repos(&owner, &repo)
            .get_ref(&Reference::Tag(tag.to_string()))
            .await?;

        let sha = match response.object {
            octocrab::models::repos::Object::Tag { sha, .. } => sha,
            _ => return Err(GitError::NoSuchTag(tag.to_string())),
        };

        Ok(sha)
    }
    /// Create a branch from some base branch in a repository
    pub async fn create_branch(
        &self,
        url: &str,
        base_branch: &str,
        new_branch: &str,
    ) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let response = self
            .octocrab
            .clone()
            .repos(&owner, &repo)
            .get_ref(&Reference::Branch(base_branch.to_string()))
            .await?;

        let sha = match response.object {
            octocrab::models::repos::Object::Commit { sha, .. } => sha,
            _ => return Err(GitError::NoSuchBranch(base_branch.to_string())),
        };

        let response = self
            .octocrab
            .clone()
            .repos(&owner, &repo)
            .create_ref(&Reference::Branch(new_branch.to_string()), sha)
            .await;

        match response {
            Ok(_) => {
                println!("Successfully created branch '{new_branch}' for {repo}");
                Ok(())
            }
            Err(e) => {
                eprintln!("Failed to create branch '{new_branch}' for {repo}: {e}");
                Err(GitError::GithubApiError(e))
            }
        }
    }
    /// Create all passed branches for each repository provided
    pub async fn create_all_branches(
        &self,
        base_branch: &str,
        new_branch: &str,
        repositories: &Vec<String>,
    ) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            futures.push(async move {
                let result = self.create_branch(repo, base_branch, new_branch).await;
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

        let result = self
            .octocrab
            .clone()
            .repos(&owner, &repo)
            .delete_ref(&Reference::Branch(branch.to_string()))
            .await;
        match result {
            Ok(()) => {
                println!("Successfully deleted branch '{branch}' for {repo}");
                Ok(())
            }
            Err(e) => {
                eprintln!("Failed to delete branch '{branch}' for {repo}: {e}");
                Err(GitError::GithubApiError(e))
            }
        }
    }
    /// Delete the specified branch for each configured repository
    pub async fn delete_all_branches(
        &self,
        branch: &str,
        repositories: &Vec<String>,
    ) -> Result<(), GitError> {
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
                Err(e) => eprintln!("Failed to delete {branch} for {repo}: {e}"),
            }
        }
        Ok(())
    }

    /// Create a tag for a specific repository
    pub async fn create_tag(&self, url: &str, tag: &str, branch: &str) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let sha = self.get_branch_sha(url, branch).await?;
        let (owner, repo) = (info.owner, info.repo_name);

        let response = self
            .octocrab
            .clone()
            .repos(&owner, &repo)
            .create_ref(&Reference::Tag(tag.to_string()), sha)
            .await;

        match response {
            Ok(_) => {
                println!("Successfully created tag '{tag}' for {repo}");
                Ok(())
            }
            Err(e) => {
                eprintln!("Failed to create tag '{tag}' for {repo}: {e}");
                Err(GitError::GithubApiError(e))
            }
        }
    }

    /// Create the tag for all configured repositories
    pub async fn create_all_tags(
        &self,
        tag: &str,
        branch: &str,
        repositories: Vec<String>,
    ) -> Result<(), GitError> {
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

        //let result = self.octocrab.clone().repos(owner, repo).delete_ref(&Reference::Tag(tag.to_string())).await;

        let response = self
            .octocrab
            .clone()
            ._delete(
                format!("/repos/{owner}/{repo}/git/refs/tags/{tag}"),
                None::<&()>,
            )
            .await;

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
                        _ => Err(GitError::Other(format!(
                            "Cannot delete {tag}: {}",
                            resp.status()
                        ))),
                    }
                }
            }
            Err(e) => Err(GitError::Other(format!("Cannot delete {tag}: {e}"))),
        }
    }
    /// Delete the specified tag for all configured repositories
    pub async fn delete_all_tags(
        &self,
        tag: &str,
        repositories: &Vec<String>,
    ) -> Result<(), GitError> {
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

    /// Generate release notes for a particular releaese. It grabs all the commits present in `tag`
    /// that are newer than the latest commit in `previous_tag`.
    /// This needs to be cleaned up, it is a bit of a mess right now.
    pub async fn generate_release_notes(
        &self,
        url: &str,
        tag: &str,
        previous_tag: &str,
    ) -> Result<ReleaseNotes, GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);

        let octocrab = self.octocrab.clone();

        let tag_info = octocrab
            .repos(&owner, &repo)
            .get_ref(&Reference::Tag(previous_tag.to_string()))
            .await?;
        let a = match tag_info.object {
            octocrab::models::repos::Object::Tag { sha, .. } => sha,
            octocrab::models::repos::Object::Commit { sha, .. } => sha,
            _ => "".to_string(),
        };
        let mut page: u32 = 1;
        let per_page = 100;
        let mut date: DateTime<Utc> = Utc::now();

        let c = octocrab
            .repos(&owner, &repo)
            .list_commits()
            .per_page(1)
            .sha(a)
            .send()
            .await?;
        if let Some(commit) = c.items.first() {
            commit
                .commit
                .committer
                .as_ref()
                .map(|c| {
                    if let Some(d) = c.date {
                        date = d;
                    } else {
                        eprintln!("Can't get date of commit, assume things are broken");
                    }
                    println!("Last commit date: {}", date.to_rfc3339());
                })
                .unwrap_or_else(|| eprintln!("No commit found"));
        }

        let mut new_commits: Vec<String> = Vec::with_capacity(200);
        loop {
            let commits = octocrab
                .repos(&owner, &repo)
                .list_commits()
                .page(page)
                .per_page(per_page)
                .since(date)
                .branch(tag.to_string())
                .send()
                .await?;

            new_commits.extend(
                commits
                    .items
                    .iter()
                    .map(|c| c.commit.message.lines().next().unwrap().to_string()),
            );

            if commits.items.len() < per_page as usize {
                break;
            }
            page += 1;
        }
        let component_upper_bracket = format!("[{}", repo.to_uppercase());
        let component_upper = repo.to_uppercase();
        let mut c = repo.chars();
        let capitalized = match c.next() {
            None => String::new(),
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        };
        let mut vulnerabilities = Vec::new();
        let mut odp_changes = Vec::new();
        let mut component_match = Vec::new();
        let mut other = Vec::new();
        for s in &new_commits {
            let s_upper = s.to_ascii_uppercase();
            if s_upper.contains("OSV") {
                vulnerabilities.push(s.clone());
            } else if s_upper.contains("ODP") {
                odp_changes.push(s.clone());
            } else if s_upper.contains(&component_upper) {
                component_match.push(s.clone());
            } else {
                other.push(s.clone());
            }
        }

        let mut body_commits: Vec<String> = Vec::with_capacity(new_commits.len() + 10);
        if !odp_changes.is_empty() {
            body_commits.push("### ODP Changes".to_string());
            for commit in odp_changes {
                body_commits.push(format!("* {commit}"));
            }
        }
        if !vulnerabilities.is_empty() {
            body_commits.push("### CVE related changes".to_string());
            for commit in vulnerabilities {
                body_commits.push(format!("* {commit}"));
            }
        }
        if !component_match.is_empty() {
            body_commits.push(format!("### Changes from {capitalized}"));
            for commit in component_match {
                body_commits.push(format!("* {commit}"));
            }
        }
        if !other.is_empty() {
            body_commits.push("### Other changes".to_string());
            for commit in other {
                body_commits.push(format!("* {commit}"));
            }
        }

        let body = body_commits.join("\n");
        let mut name = tag.replace("-tag", "");
        name = name.replace("ODP-", "");
        name = format!("# Release Notes for {capitalized} {name}");
        println!("{name}");
        println!("{body}");
        let notes: ReleaseNotes = ReleaseNotes { name, body };

        Ok(notes)
    }

    /// Determine if a specific branch is protected
    async fn get_branch_protection(
        &self,
        url: &str,
        branch: &str,
    ) -> Result<CheckResult, GitError> {
        let info = get_repo_info_from_url(url)?;
        let get_url = format!(
            "/repos/{}/{}/branches/{branch}/protection",
            info.owner, info.repo_name
        );
        let response: Result<serde_json::Value, octocrab::Error> =
            self.octocrab.clone().get(get_url, None::<&()>).await;
        handle_api_response!(
            response,
            format!(
                "Unable to get branch protection for {}/{}:{}",
                info.owner, info.repo_name, branch
            ),
            |body: serde_json::Value| {
                let enabled: bool = body
                    .get("enabled")
                    .and_then(|e| e.as_bool())
                    .unwrap_or(false);
                Ok(CheckResult::Protected(enabled))
            },
        )
    }

    /// Run the selected checks against a specific repository
    pub async fn check_repository(
        &self,
        url: &str,
        checks: &RepoChecks,
        blacklist: &HashSet<String>,
    ) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let (protected, license, (old, days_ago)) =
            (checks.protected, checks.license, checks.old_branches);

        println!("Checking repository {owner}/{repo}");
        let mut futures = FuturesUnordered::new();
        if protected {
            let url = info.url.clone();
            let branch = info
                .main_branch
                .clone()
                .unwrap_or_else(|| "main".to_string());
            futures.push(
                async move { self.get_branch_protection(url.as_str(), &branch).await }.boxed(),
            );
        }

        if license {
            let url = info.url.clone();
            futures.push(async move { self.get_license(url.as_str()).await }.boxed());
        }

        if old {
            let url = info.url.clone();
            futures.push(
                async move {
                    self.get_old_branches(
                        url.as_str(),
                        days_ago,
                        blacklist.clone(),
                        &checks.branch_filter,
                    )
                    .await
                    .map(CheckResult::OldBranches)
                }
                .boxed(),
            )
        }

        let mut errors = Vec::new();
        while let Some(result) = futures.next().await {
            match result {
                Ok(CheckResult::License(license)) => {
                    println!("License for {owner}/{repo} is '{license}'")
                }
                Ok(CheckResult::Protected(true)) => {
                    println!("Main branch for {owner}/{repo} is protected")
                }
                Ok(CheckResult::Protected(false)) => {
                    eprintln!("Main branch for {owner}/{repo} is not protected");
                    errors.push(GitError::NoMainBranchProtection(format!("{owner}/{repo}")));
                }
                Ok(CheckResult::OldBranches(branches)) => {
                    if !branches.is_empty() {
                        if branches.len() == 1 {
                            println!(
                                "Found {} matching old branch in {owner}/{repo}",
                                branches.len()
                            );
                        } else {
                            println!(
                                "Found {} matching old branches in {owner}/{repo}",
                                branches.len()
                            );
                        }
                        let mut builder = Builder::default();
                        builder.push_record(vec!["Name", "Last Modified"]);
                        for (name, date) in branches {
                            builder.push_record(vec![name, date]);
                        }

                        let mut table = builder.build();

                        table.with(
                            Style::ascii()
                                .horizontals([(1, HorizontalLine::inherit(Style::ascii()))]),
                        );
                        table.with((Alignment::center(), Padding::new(1, 1, 0, 0)));
                        println!("{table}");
                    }
                }
                Err(e) => {
                    eprintln!("Check failed for {owner}/{repo}: {e}");
                    errors.push(e);
                }
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(())
    }
    /// Run the selected checks against all configured repositories
    pub async fn check_all_repositories(
        &self,
        repositories: Vec<String>,
        checks: &RepoChecks,
        blacklist: &HashSet<String>,
    ) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            futures.push(async move { self.check_repository(&repo, checks, blacklist).await });
        }

        let mut errors = Vec::new();
        while let Some(result) = futures.next().await {
            match result {
                Ok(_) => {}
                Err(e) => {
                    errors.push(e);
                }
            }
        }
        if !errors.is_empty() {
            Err(GitError::MultipleErrors(errors))
        } else {
            Ok(())
        }
    }

    /// Get the license for a repository
    async fn get_license(&self, url: &str) -> Result<CheckResult, GitError> {
        let info = get_repo_info_from_url(url)?;
        let content = self
            .octocrab
            .clone()
            .repos(&info.owner, &info.repo_name)
            .license()
            .await?;
        if let Some(license) = content.license {
            Ok(CheckResult::License(license.name))
        } else {
            Err(GitError::MissingLicense(url.to_string()))
        }
    }
    /// Get branches that are older than a certain number of days
    /// A blacklist should be passed to ignore certain branches, since some of them are static and
    /// will never change.
    async fn get_old_branches(
        &self,
        url: &str,
        days_ago: i64,
        blacklist: HashSet<String>,
        branch_filter: &Option<Regex>,
    ) -> Result<Vec<(String, String)>, GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let per_page = 100;
        let mut page: u32 = 1;
        let octocrab = self.octocrab.clone();
        let mut all_branches: Vec<String> = Vec::new();
        loop {
            let branches = octocrab
                .repos(&owner, &repo)
                .list_branches()
                .per_page(per_page)
                .page(page)
                .send()
                .await?;

            all_branches.extend(branches.items.iter().filter_map(|b| {
                if branch_filter
                    .as_ref()
                    .map(|re| re.is_match(b.name.as_str()))
                    .unwrap_or(true)
                {
                    Some(b.name.clone())
                } else {
                    None
                }
            }));
            if branches.items.len() < per_page as usize {
                break;
            }
            page += 1;
        }
        let mut ages = HashMap::<String, DateTime<Utc>>::new();
        let now = Utc::now();
        for branch in all_branches {
            let commits = octocrab
                .repos(&owner, &repo)
                .list_commits()
                .sha(&branch)
                .per_page(1)
                .send()
                .await?;

            if let Some(commit) = commits.items.first() {
                if let Some(commit_date) = commit.commit.committer.as_ref().and_then(|c| c.date) {
                    if let Some(re) = branch_filter {
                        re.is_match(&branch)
                            .then(|| branch.clone())
                            .map(|b| ages.insert(b, commit_date));
                    } else {
                        ages.insert(branch.clone(), commit_date);
                    }
                } else {
                    eprintln!("No commit date found for branch {branch}");
                }
            }
        }
        let mut old_branches: Vec<_> = ages
            .into_iter()
            .filter(|(_, age)| (now - age).num_days() >= days_ago)
            .filter(|(branch, _)| !blacklist.contains(branch))
            .collect();
        old_branches.sort_by_key(|(_, age)| *age);

        let old_branches: Vec<(String, String)> = old_branches
            .into_iter()
            .map(|(branch, age)| (branch, age.format("%Y-%m-%d").to_string()))
            .collect();

        Ok(old_branches)
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
