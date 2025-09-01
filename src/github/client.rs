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
use crate::config::Config;
use crate::error::{GitError, is_retryable};
use crate::github::tag::RepoResponse;
use crate::utils::pr::{CreatePrOptions, MergePrOptions};
use crate::utils::repo::{
    BranchProtectionRule, Checks, LicenseInfo, RepoChecks, RepoInfo, TagInfo, TagType,
    get_repo_info_from_url, http_to_ssh_repo,
};
use crate::{async_retry, handle_api_response, handle_futures_unordered};
use chrono::{DateTime, Duration, Local, TimeZone, Utc};
use futures::{FutureExt, StreamExt, future::try_join, stream::FuturesUnordered};
use indicatif::{ProgressBar, ProgressStyle};
use octocrab::Octocrab;
use octocrab::models::repos::{Ref, ReleaseNotes, Tag};
use octocrab::params::repos::Reference;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::sync::OnceLock;
use tokio_retry::{
    RetryIf,
    strategy::{ExponentialBackoff, jitter},
};
/*use tabled::{
    builder::Builder,
    settings::{
        Alignment, Padding,
        style::{HorizontalLine, Style},
    },
};
*/
use temp_dir::TempDir;

use indexmap::IndexSet;
use serde_json::json;

// Static, pre-compiled at first use:
static CVE_REGEX: OnceLock<Regex> = OnceLock::new();
static ODP_REGEX: OnceLock<Regex> = OnceLock::new();

/// Contains information about tags for a forked repo, its parent,
/// and the tags that are missing from the fork
#[derive(Debug)]
pub struct Comparison {
    pub fork_tags: IndexSet<TagInfo>,
    pub parent_tags: IndexSet<TagInfo>,
    pub missing_in_fork: IndexSet<TagInfo>,
}
/*enum Output {
    Stdout,
    Stderr,
}
*/

/// Github api entry point
pub struct GithubClient {
    /// Octocrab client. This can be trivially cloned
    pub octocrab: Octocrab,
    is_a_tty: bool,
}

impl GithubClient {
    pub fn new(github_token: String, _config: &Config, is_a_tty: bool) -> Result<Self, GitError> {
        let octocrab = Octocrab::builder()
            .personal_token(github_token.clone())
            .build()
            .map_err(GitError::GithubApiError)?;
        println!("Is user? {is_a_tty}");
        Ok(Self { octocrab, is_a_tty })
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

    /// This can be used to fetch tags in a more api-call efficient way than using the rest api.
    /// It does mean we have to manually query the graphql endpoint and manually parse the json
    /// output, rather than having it done for us by octocrab.
    /// We can't actually get all the information required for an annotated tag, but we can use it
    /// to distinguish between lightweight and annotated tags. If we don't get any annotated tags,
    /// we can skip the fairly slow git clone and push process
    ///
    /// `IndexSet` is an implementation of an orderered Set.
    pub async fn get_tags(&self, url: &str) -> Result<IndexSet<TagInfo>, GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        //let mut all_tags: Vec<TagInfo> = Vec::new();
        let mut all_tags: IndexSet<TagInfo> = IndexSet::new();
        let mut has_next_page = true;
        let mut after: Option<String> = None;
        let per_page = 100;

        //let octocrab = self.octocrab.clone();
        let query = r#"
        query($owner: String!, $repo: String!, $first: Int!, $after: String) {
            repository(owner: $owner, name: $repo) {
                parent {
                    url
                }
                refs(refPrefix: "refs/tags/", first: $first, after: $after) {
                    nodes {
                        name
                        target {
                            __typename
                            oid
                            ... on Tag {
                                message
                                tagger {
                                    name
                                    email
                                    date
                                }
                            }
                        }
                    }
                    pageInfo {
                        hasNextPage
                        endCursor
                    }
                }
            }
        }
        "#;

        while has_next_page {
            let payload = json!({
                "query": query,
                "variables": {
                    "owner": owner,
                    "repo": repo,
                    "first": per_page,
                    "after": after,
                }
            });
            let res: RepoResponse = self.octocrab.graphql(&payload).await?;
            let repo = &res.data.repository;
            let parent_url = repo.parent.as_ref().map(|p| p.url.clone());

            for tag in &repo.refs.nodes {
                let tag_type = match tag.target.typename.as_str() {
                    "Tag" => TagType::Annotated,
                    "Commit" => TagType::Lightweight,
                    other => return Err(GitError::Other(format!("Unknown tag type '{other}'"))),
                };

                let (message, tagger_name, tagger_email, tagger_date) =
                    if let Some(tagger) = &tag.target.tagger {
                        (
                            tag.target.message.clone(),
                            tagger.name.clone(),
                            tagger.email.clone(),
                            tagger.date.clone(),
                        )
                    } else {
                        (None, None, None, None)
                    };

                let ssh_url = http_to_ssh_repo(url)?;

                all_tags.insert(TagInfo {
                    name: tag.name.clone(),
                    tag_type,
                    sha: tag.target.oid.clone(),
                    message,
                    tagger_name,
                    tagger_email,
                    tagger_date,
                    parent_url: parent_url.clone(),
                    ssh_url,
                });
            }
            has_next_page = repo.refs.page_info.has_next_page;
            after = repo.refs.page_info.end_cursor.clone();
        }

        Ok(all_tags)
    }
    pub async fn compare_tags(&self, url: &str, parent: &RepoInfo) -> Result<Comparison, GitError> {
        let (fork_tags, parent_tags) =
            try_join(self.get_tags(url), self.get_tags(&parent.url)).await?;
        println!(
            "Fork tags: {}\nParent tags: {}",
            fork_tags.len(),
            parent_tags.len()
        );

        let missing_in_fork: IndexSet<TagInfo> =
            parent_tags.difference(&fork_tags).cloned().collect();
        let compare = Comparison {
            fork_tags,
            parent_tags,
            missing_in_fork,
        };
        Ok(compare)
    }
    /// Sync many tags asynchronously. We can use the github api to sync lightweight tags, but to
    /// sync annotated tags we need to use git. Unfortunately there's no way around that.
    pub async fn sync_tags(&self, url: &str, process_annotated: bool) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let parent = self.get_parent_repo(url).await?;
        let (owner, repo) = (info.owner, info.repo_name);
        let missing = self.compare_tags(url, &parent).await?.missing_in_fork;
        if missing.is_empty() {
            println!("No missing tags in {url}");
            return Ok(());
        }

        let lightweight: IndexSet<TagInfo> = missing
            .iter()
            .filter(|t| t.tag_type == TagType::Lightweight)
            .cloned()
            .collect();

        let annotated: IndexSet<TagInfo> = missing
            .into_iter()
            .filter(|t| t.tag_type == TagType::Annotated)
            .collect();

        let lightweight_fut = async {
            handle_futures_unordered!(
                lightweight.into_iter().map(|tag| {
                    let owner = owner.clone();
                    let repo = repo.clone();
                    let name = tag.name.clone();
                    (repo, name, owner, tag)
                }),
                |repo, name, owner, tag| self.sync_lightweight_tag(&owner, &repo.clone(), &tag).map(|result|(repo, name, result)),
                (repo, name, result) {
                    match result {
                        Ok(_) => println!("Successfully synced tag '{name}' in '{repo}'"),
                        Err(e) => eprintln!("Failed to sync '{name}' for '{repo}': {e}")
                    }
                }
            );
            Ok::<(), GitError>(())
        };
        // Run both of these at the same time. Annotated tags are much slower to sync than
        // ligthweight tags since we need to clone, fetch from upstream, then push.
        if process_annotated {
            let ssh_url = http_to_ssh_repo(url)?;
            let output = tokio::join!(
                self.sync_annotated_tags(&annotated, &parent.url, &ssh_url),
                lightweight_fut,
            );
            let (annotated, lightweight) = output;
            if annotated.is_err() {
                eprintln!(
                    "Failed to sync annotated tags for {owner}/{repo}: {}",
                    annotated.err().unwrap()
                );
            }
            if lightweight.is_err() {
                eprintln!(
                    "Failed to sync lightweight tags for {owner}/{repo}: {}",
                    lightweight.err().unwrap()
                );
            }
        } else {
            lightweight_fut.await?;
        }

        Ok(())
    }
    pub async fn sync_all_tags(
        &self,
        process_annotated: bool,
        repositories: Vec<String>,
    ) -> Result<(), GitError> {
        handle_futures_unordered!(
            repositories.into_iter().map(|url| {
                let url = url.clone();
                let info = get_repo_info_from_url(&url).unwrap();
                let (owner, repo) = (info.owner, info.repo_name);
                (owner, repo, url)
            }),
            |owner, repo , url| self.sync_tags(&url.clone(), process_annotated).map(move |result| (url, owner, repo, result)),
            (url, owner, repo, result) {
                match result {
                    Ok(_) => println!("Successfully synced tags for {owner}/{repo}: {url}"),
                    Err(e) => eprintln!("Failed to sync tags for repo {owner}/{repo}: {e}"),
                }
            }
        );

        Ok(())
    }
    /// Sync lightweight tags from the parent repo to the forked repo. This can be trivially done
    /// using the github api, so we don't need to call out to
    pub async fn sync_lightweight_tag(
        &self,
        owner: &str,
        repo: &str,
        tag: &TagInfo,
    ) -> Result<(), GitError> {
        let body = json!({
            "ref": format!("refs/tags/{}", tag.name),
            "sha": tag.sha,
        });
        let response: Result<serde_json::Value, octocrab::Error> = self
            .octocrab
            .clone()
            .post::<serde_json::Value, _>(format!("/repos/{owner}/{repo}/git/refs"), Some(&body))
            .await;

        handle_api_response!(
            response,
            format!("Unable to sync tag {} in {owner}/{repo}", tag.name),
            |_| {
                println!(
                    "Successfully synced lightweight tag '{}' {owner}/{repo}",
                    tag.name
                );
                Ok::<(), GitError>(())
            },
        )?;
        Ok(())
    }

    /// Sync all new annotated tags from a forked repo with its parent.
    /// Doing this *requires* using git (or some re-implementation of git). Syncing annotated tags
    /// through the github api with all of its fields, including signing, is currently not
    /// possible.
    pub async fn sync_annotated_tags(
        &self,
        tags: &IndexSet<TagInfo>,
        parent_url: &str,
        ssh_url: &str,
    ) -> Result<(), GitError> {
        if tags.is_empty() {
            return Ok(());
        }
        println!("{tags:#?}");

        // Use a temp directory for the git repository so it's cleaned up automatically
        let tmp_dir = TempDir::new()
            .map_err(|e| GitError::Other(format!("Failed to create temp dir: {e}")))?;
        let tmp = tmp_dir.path();
        let tmp_str = tmp
            .to_str()
            .ok_or_else(|| GitError::Other("Temp dir not valid UTF-8".to_string()))?;

        println!("{parent_url} - {ssh_url}");

        // Clone with the bare minimum information to reduce the amount we download
        Command::new("git")
            .args([
                "clone",
                "--bare",
                "--filter=blob:none",
                "--depth=1",
                ssh_url,
                tmp_str,
            ])
            .status()?;

        let output = Command::new("git")
            .args(["-C", tmp_str, "remote", "get-url", "upstream"])
            .output()?;
        if output.status.success() {
            Command::new("git")
                .args(["-C", tmp_str, "remote", "set-url", "upstream", parent_url])
                .status()?;
        } else {
            Command::new("git")
                .args(["-C", tmp_str, "remote", "add", "upstream", parent_url])
                .status()?;
        }

        // Only fetch the annotated tags that we're interested in adding to our fork.
        // Lightweight tags can be synced automatically with github
        let mut fetch_args = vec![
            "-C",
            tmp_str,
            "fetch",
            "--filter=blob:none",
            "--depth=1",
            "upstream",
        ];
        for tag in tags {
            fetch_args.push("tag");
            fetch_args.push(tag.name.as_str());
        }

        Command::new("git").args(&fetch_args).status()?;

        // Only push the newly added annotated tags
        let mut push_args = vec!["-C", tmp_str, "push", "origin"];
        push_args.extend(tags.iter().map(|tag| tag.name.as_str()));

        Command::new("git").args(&push_args).status()?;
        Ok(())
    }

    /// Get a diff of tags between a single forked repository and its parent repository.
    pub async fn diff_tags(&self, url: &str) -> Result<Comparison, GitError> {
        let parent = self.get_parent_repo(url).await?;
        let comparison = self.compare_tags(url, &parent).await?;

        println!(
            "Fork has {} tags, parent has {} tags",
            comparison.fork_tags.len(),
            comparison.parent_tags.len()
        );

        /*if !comparison.missing_in_fork.is_empty() {
            println!(
                "\nTags missing in fork: {}",
                comparison.missing_in_fork.len()
            );
            for (name, _) in &comparison.missing_in_fork {
                //println!(" - {name}");
            }
        } else {
            println!("{repo} up to date");
        }
        */

        Ok(comparison)
    }
    /// Get a diff of all configured repositories tags, compared against their parent.
    pub async fn diff_all_tags(&self, repositories: Vec<String>) -> Result<(), GitError> {
        //let mut futures = FuturesUnordered::new();
        let repositories: Vec<Result<RepoInfo, _>> = repositories
            .iter()
            .map(|url| get_repo_info_from_url(url))
            .collect();

        let mut diffs: HashMap<String, Comparison> = HashMap::new();

        let pb = create_progress_bar(repositories.len() as u64);

        let mut errors: Vec<(String, GitError)> = Vec::new();
        handle_futures_unordered!(
            repositories.into_iter().flatten().map(|repo|{
                let url = repo.url.clone();
                let owner = repo.owner.clone();
                let repo_name = repo.repo_name.clone();
                self.println(&format!("   Processing {owner}/{repo_name}"), &pb);
                (owner, repo_name, url )
            }),
            |owner, repo, url| self.diff_tags(&url).map(|result| (owner, repo, result)),
            (owner, repo, result) {
                match result {
                    Ok(r) => {
                        diffs.insert(repo.to_string(), r);
                        if let Some(pb) = &pb {
                            pb.inc(1);
                        }
                        self.println(&format!("✅ Successfully diffed {owner}/{repo}"), &pb);
                    },
                    Err(e) => {
                        if let Some(pb) = &pb {
                            pb.inc(1);
                        }
                        self.println(&format!("❌ {owner}/{repo}: {e}"), &pb);
                        errors.push((format!("{owner}/{repo}"), e));
                    }
                }
            }
        );
        if let Some(pb) = pb {
            pb.finish_with_message("All repositories processed");
        }
        if !diffs.is_empty() {
            println!("\nTags missing in forks:");
        }
        for (name, comparison) in diffs {
            /*if !comparison.missing_in_fork.is_empty() {
                println!("# {name}:");
            }
            for comp in comparison.missing_in_fork {
                println!("\t{}", comp.name);
            }
            */
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
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
    pub async fn sync_all_forks(&self, repositories: Vec<String>) -> Result<(), GitError> {
        let pb = create_progress_bar(repositories.len() as u64);

        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            self.println(&format!("   Processing {repo}"), &pb);
            futures.push(async move {
                let result = self.sync_fork(&repo).await;
                (repo, result)
            });
        }
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(_) => {
                    if let Some(pb) = &pb {
                        pb.inc(1);
                    }
                    self.println(&format!("✅ Successfully synced {repo}"), &pb);
                }
                Err(e) => {
                    if let Some(pb) = &pb {
                        pb.inc(1);
                    }
                    self.println(&format!("❌ Failed to sync {repo}: {e}"), &pb);
                    errors.push((repo.to_string(), e));
                }
            }
        }
        if let Some(pb) = pb {
            pb.finish_with_message("All repositories processed");
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(())
    }
    /// Get the most recent commit of a branch, so we can use that to create and delete it
    async fn get_branch_sha(&self, url: &str, branch: &str) -> Result<String, GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let res: Result<_, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                self.octocrab
                    .clone()
                    .repos(&owner, &repo)
                    .get_ref(&Reference::Branch(branch.to_string()))
                    .await
            },
        );

        match res {
            Ok(r) => {
                let sha = match r.object {
                    octocrab::models::repos::Object::Commit { sha, .. } => sha,
                    _ => return Err(GitError::NoSuchBranch(branch.to_string())),
                };
                Ok(sha)
            }
            Err(e) => Err(GitError::GithubApiError(e)),
        }
    }
    /// Get the sha of a tag
    #[allow(dead_code)]
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
                //println!("Successfully created branch '{new_branch}' for {repo}");
                Ok(())
            }
            Err(e) => {
                //eprintln!("Failed to create branch '{new_branch}' for {repo}: {e}");
                Err(GitError::GithubApiError(e))
            }
        }
    }
    /// Create the passed branch for each repository provided
    pub async fn create_all_branches(
        &self,
        base_branch: &str,
        new_branch: &str,
        repositories: &Vec<String>,
    ) -> Result<(), GitError> {
        let pb = create_progress_bar(repositories.len() as u64);
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            self.println(&format!("   Processing {repo}"), &pb);

            futures.push(async move {
                let result = self.create_branch(repo, base_branch, new_branch).await;
                (repo, result)
            });
        }

        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(_) => {
                    if let Some(pb) = &pb {
                        pb.inc(1);
                    }
                    self.println(
                        &format!("✅ Successfully created '{new_branch}' for {repo}"),
                        &pb,
                    );
                }
                Err(e) => {
                    if let Some(pb) = &pb {
                        pb.inc(1);
                    }
                    self.eprintln(
                        &format!("❌ Failed to create '{new_branch}' for {repo}"),
                        &pb,
                    );
                    errors.push((repo.to_string(), e));
                }
            }
        }
        if let Some(pb) = &pb {
            pb.finish_with_message("All repositories processed");
        }

        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
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
                println!("✅ Successfully deleted branch '{branch}' for {repo}");
                Ok(())
            }
            Err(e) => {
                eprintln!("❌ Failed to delete branch '{branch}' for {repo}: {e}");
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
        let pb = create_progress_bar(repositories.len() as u64);
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            self.println(&format!("   Processing {repo}"), &pb);
            self.println(&format!("   Processing {repo}"), &pb);
            futures.push(async move {
                let result = self.delete_branch(repo, branch).await;
                (repo, result)
            });
        }

        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(_) => {
                    if let Some(pb) = &pb {
                        pb.inc(1);
                    }
                    self.println(
                        &format!("✅ Successfully deleted '{branch}' for {repo}"),
                        &pb,
                    );
                }
                Err(e) => {
                    if let Some(pb) = &pb {
                        pb.inc(1);
                    }
                    self.eprintln(&format!("❌ Failed to delete '{branch}' for {repo}"), &pb);
                    errors.push((format!("{repo} ({branch})"), e));
                }
            }
        }
        if let Some(pb) = &pb {
            pb.finish_with_message("All repositories processed");
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(())
    }

    /// Create a tag for a specific repository
    pub async fn create_tag(&self, url: &str, tag: &str, branch: &str) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let sha = self.get_branch_sha(url, branch).await?;
        let (owner, repo) = (info.owner, info.repo_name);

        let res: Result<Ref, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                self.octocrab
                    .clone()
                    .repos(&owner, &repo)
                    .create_ref(&Reference::Tag(tag.to_string()), sha.clone())
                    .await
            },
        );

        match res {
            Ok(_) => {
                println!("Successfully created tag '{tag}' for {repo}");
                Ok(())
            }
            Err(e) => Err(GitError::GithubApiError(e)),
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

    /// Create a new release for a specific repository. Release notes will also be generated.
    pub async fn create_release(
        &self,
        url: &str,
        current_tag: &str,
        previous_tag: &str,
        release_name: Option<&str>,
    ) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);

        let release_notes = self
            .generate_release_notes(url, current_tag, previous_tag)
            .await?;

        let name = if let Some(release_name) = release_name {
            release_name.to_string()
        } else {
            current_tag.to_string()
        };
        // ExponentialBackoff, with a max of three tries in case something fails for a tarnsient
        // reason.
        let retries = 3;
        let retry_strategy = ExponentialBackoff::from_millis(100)
            .map(jitter)
            .take(retries);

        let release = RetryIf::spawn(
            retry_strategy.clone(),
            || async {
                self.octocrab
                    .clone()
                    .repos(&owner, &repo)
                    .releases()
                    .create(current_tag)
                    .name(&name)
                    .body(&release_notes.body)
                    .send()
                    .await
            },
            |e: &octocrab::Error| is_retryable(e),
        )
        .await;

        match release {
            Ok(_) => {
                println!("Successfully created release '{name}' for {repo}");
                Ok(())
            }
            Err(e) => Err(GitError::GithubApiError(e)),
        }
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
        let tag_sha = match tag_info.object {
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
            .sha(tag_sha)
            .send()
            .await?;
        if let Some(commit) = c.items.first() {
            commit
                .commit
                .committer
                .as_ref()
                .map(|c| {
                    if let Some(d) = c.date {
                        // Set the oldest commits we're going to look for to just a little bit
                        // after the most recent commit in the old tag, so that we get no overlap
                        date = d + Duration::days(1);
                    } else {
                        eprintln!("Can't get date of commit, assume things are broken");
                    }
                    println!("Last commit date: {}", date.to_rfc3339());
                })
                .unwrap_or_else(|| eprintln!("No commit found"));
        }

        // This is an arbitrary pre-allocation that should speed up pushing to the vec. Most of the
        // time we can assume that we'll get fewer than 300 commits difference between two
        // branches/tags. If not, pushing elements after 300 will just take slightly longer.
        let mut new_commits: Vec<String> = Vec::with_capacity(300);
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
            // Arbitrary pre-allocated length. Uses a bit more memory, but reduces
            // the number of potential resizes
            let mut commit_line = String::with_capacity(200);
            for commit in &commits.items {
                commit_line.clear();
                if let Some(first_line) = commit.commit.message.lines().next() {
                    commit_line.push_str(first_line);
                    new_commits.push(commit_line.clone());
                }
            }
            /*new_commits.extend(
                commits
                    .items
                    .iter()
                    .map(|c| c.commit.message.lines().next().unwrap().to_string()),
            );
            */

            if commits.items.len() < per_page as usize {
                break;
            }
            page += 1;
        }
        let mut c = repo.chars();
        let capitalized = match c.next() {
            None => String::new(),
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        };
        let mut vulnerabilities = Vec::new();
        let mut odp_changes = Vec::new();
        let mut component_match = Vec::new();
        let mut other = Vec::new();

        println!("Upper case: {}", repo.to_ascii_uppercase());
        let re = Regex::new(&format!("{}-[0-9]+", repo.to_ascii_uppercase()))?;
        let cve =
            CVE_REGEX.get_or_init(|| Regex::new(r"OSV|CVE").expect("Failed to compile CVE regex"));
        let odp = ODP_REGEX
            .get_or_init(|| Regex::new(r"ODP-[0-9]*").expect("Failed to compile ODP regex"));

        for s in &new_commits {
            let s_upper = s.to_ascii_uppercase();
            if cve.is_match(&s_upper) {
                vulnerabilities.push(s.clone());
            // We don't want to match not upper case names for components, so take the line as is
            } else if re.is_match(s) {
                component_match.push(s.clone());
            } else if odp.is_match(&s_upper) {
                odp_changes.push(s.clone());
            } else {
                other.push(s.clone());
            }
        }

        /*let mut body_header = tag.replace("-tag", "");
        body_header = body_header.replace("ODP-", "");
        body_header = format!("# Release Notes for {capitalized} {body_header}");
        */

        let body_header = format!(
            "# Release Notes for {capitalized} {}",
            tag.strip_suffix("-tag")
                .unwrap_or(tag)
                .strip_prefix("ODP-")
                .unwrap_or_else(|| tag.strip_suffix("-tag").unwrap_or(tag))
        );

        let mut body_commits: Vec<String> = Vec::with_capacity(new_commits.len() + 10);
        body_commits.push(body_header.clone());
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
        let notes: ReleaseNotes = ReleaseNotes {
            name: body_header,
            body,
        };

        Ok(notes)
    }

    /// Create a pull request for a specific repository
    pub async fn create_pr(&self, opts: &CreatePrOptions) -> Result<u64, GitError> {
        let info = get_repo_info_from_url(&opts.url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let octocrab = self.octocrab.clone();

        // ExponentialBackoff, with a max of three tries in case something fails for a tarnsient
        // reason.
        let retries = 3;
        let retry_strategy = ExponentialBackoff::from_millis(100)
            .map(jitter)
            .take(retries);
        let pr_number: u64;
        let pr_result = RetryIf::spawn(
            retry_strategy.clone(),
            || async {
                octocrab
                    .clone()
                    .pulls(&owner, &repo)
                    .create(&opts.title, &opts.head, &opts.base)
                    .body(opts.body.as_deref().unwrap_or_default())
                    .send()
                    .await
            },
            |e: &octocrab::Error| is_retryable(e),
        )
        .await;

        match pr_result {
            Ok(p) => {
                pr_number = p.number;
                println!("PR #{pr_number} created successfully for {owner}/{repo}")
            }
            Err(e) => {
                eprintln!("Failed to create PR for {owner}/{repo} after {retries} tries: {e}");
                return Err(GitError::GithubApiError(e));
            }
        }

        if let Some(reviewers) = opts.reviewers.as_deref() {
            let reviewer_result = RetryIf::spawn(
                retry_strategy,
                || async {
                    octocrab
                        .clone()
                        .pulls(&owner, &repo)
                        .request_reviews(pr_number, reviewers, &[])
                        .await
                },
                |e: &octocrab::Error| is_retryable(e),
            )
            .await;
            match reviewer_result {
                Ok(_) => println!(
                    "Successfully requested reviewers for PR #{pr_number} in {owner}/{repo}"
                ),
                Err(e) => eprintln!(
                    "Failed to request reviewers for PR #{pr_number} in {owner}/{repo}: {e}"
                ),
            }
        }
        Ok(pr_number)
    }
    /// Create a pull request for all configured repositories, and optionally merge them
    /// automatically, if possible
    pub async fn create_all_prs(
        &self,
        opts: &CreatePrOptions,
        merge_opts: Option<MergePrOptions>,
        repositories: Vec<String>,
    ) -> Result<HashMap<String, u64>, GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in &repositories {
            let merge_opts = merge_opts.clone();
            // Copy the fields of the opts struct, except for what we need to override (namely, the
            // url)
            let pr_opts = CreatePrOptions {
                url: repo.to_string(),
                ..opts.clone()
            };
            futures.push(async move {
                let result = self.create_pr(&pr_opts).await;

                match result {
                    Ok(pr_number) => {
                        if let Some(mut opts) = merge_opts {
                            opts.url = repo.clone();
                            opts.pr_number = pr_number;
                            let merge_result = self.merge_pr(&opts).await;
                            (repo, merge_result.map(|_| pr_number))
                        } else {
                            (repo, Ok(pr_number))
                        }
                    }
                    Err(e) => (repo, Err(e)),
                }
            });
        }

        // Keep track of which PR number belongs to which repository
        let mut pr_map: HashMap<String, u64> = HashMap::new();
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(pr_number) => {
                    pr_map.insert(repo.to_string(), pr_number);
                }
                Err(e) => errors.push((repo.to_string(), e)),
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }

        Ok(pr_map)
    }

    /// Merge a pr
    pub async fn merge_pr(&self, opts: &MergePrOptions) -> Result<(), GitError> {
        let info = get_repo_info_from_url(&opts.url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let pr_number = opts.pr_number;
        // ExponentialBackoff, with a max of three tries in case something fails for a tarnsient
        // reason.
        let retries = 3;
        let retry_strategy = ExponentialBackoff::from_millis(100)
            .map(jitter)
            .take(retries);

        let merge_result = RetryIf::spawn(
            retry_strategy.clone(),
            || async {
                self.octocrab
                    .clone()
                    .pulls(&owner, &repo)
                    .merge(opts.pr_number)
                    .message(opts.message.as_deref().unwrap_or_default())
                    .sha(opts.sha.as_deref().unwrap_or_default())
                    .method(opts.method)
                    .send()
                    .await
            },
            |e: &octocrab::Error| is_retryable(e),
        )
        .await;

        match merge_result {
            Ok(_) => {
                println!("Successfully merged PR #{pr_number} in {repo}");
                Ok(())
            }
            Err(_) => Err(GitError::PRNotMergeable(pr_number)),
        }
    }

    // Merge all PRs in the provided repositories
    pub async fn merge_all_prs(
        &self,
        opts: MergePrOptions,
        repositories: HashMap<String, u64>,
    ) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for (repo, pr_number) in repositories.into_iter() {
            let merge_opts = MergePrOptions {
                url: repo.clone(),
                pr_number,
                ..opts.clone()
            };

            futures.push(async move {
                let result = self.merge_pr(&merge_opts).await;
                (repo, result)
            });
        }

        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(_) => println!("Successfully merged PR #{} in {repo}", opts.pr_number),
                Err(e) => eprintln!("Failed to merge PR #{} in {repo}: {e}", opts.pr_number),
            }
        }
        Ok(())
    }

    /// Run the selected checks against all configured repositories
    pub async fn check_all_repositories(
        &self,
        repositories: Vec<String>,
        checks: &RepoChecks,
        blacklist: &HashSet<String>,
    ) -> Result<Vec<Checks>, GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            futures.push(async move {
                let response = self
                    .check_repository(&repo, blacklist.clone(), checks)
                    .await;
                (response, repo)
            });
        }

        let mut results: Vec<Checks> = Vec::new();
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((result, repo)) = futures.next().await {
            match result {
                Ok(r) => results.push(r),
                Err(e) => errors.push((repo, e)),
            }
        }
        if !errors.is_empty() {
            Err(GitError::MultipleErrors(errors))
        } else {
            Ok(results)
        }
    }

    /// Get branches that are older than a certain number of days
    /// A blacklist should be passed to ignore certain branches, since some of them are static and
    /// will never change, nor should they be deleted.
    pub async fn check_repository(
        &self,
        url: &str,
        blacklist: HashSet<String>,
        checks: &RepoChecks,
    ) -> Result<Checks, GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let octocrab = self.octocrab.clone();
        let (get_branches, days_ago) = checks.old_branches;
        let get_protection = checks.protected;
        let get_license = checks.license;
        let branch_filter = checks.branch_filter.clone();

        let mut protection_rules: Vec<BranchProtectionRule> = Vec::new();
        let mut license: Option<LicenseInfo>;
        // Use a graphql query to drastically reduce the number of api calls we need to make.
        // This lets us get the repo name and latest commit date in one call, instead of two.
        // This makes a huge difference for very large repositories with many branches.
        let query = r#"
            query($owner: String!, $repo: String!, $after: String, $getLicenseInfo: Boolean!, $getBranches: Boolean!, $getProtection: Boolean!) {
                repository(owner: $owner, name: $repo) {
                    licenseInfo @include(if: $getLicenseInfo) {
                        name
                        spdxId
                        url
                    }
                    branchProtectionRules(first: 100) @include(if: $getProtection){
                        nodes {
                            pattern
                            isAdminEnforced
                            requiresApprovingReviews
                            requiredApprovingReviewCount
                            requiresStatusChecks
                            requiresStrictStatusChecks
                            requiresConversationResolution
                            restrictsPushes
                            restrictsReviewDismissals
                        }
                    }

                    refs(refPrefix: "refs/heads/", first: 100, after: $after) @include(if: $getBranches) {
                        nodes {
                            name
                            target {
                                ... on Commit {
                                    committedDate
                                }
                            }
                        }
                        pageInfo {
                            hasNextPage
                            endCursor
                        }
                    }
                }
            }
        "#;

        let mut branches = HashMap::<String, DateTime<Utc>>::new();

        // Needed for the graphql query
        let mut after: Option<String> = None;

        loop {
            // Paginated results, so we have to loop over until there aren't any more pages left
            let payload = serde_json::json!({
                "query": query,
                "variables": {
                    "owner": owner,
                    "repo": repo,
                    "after": after,
                    "getBranches": get_branches,
                    "getProtection": get_protection,
                    "getLicenseInfo": get_license,
                }
            });

            let response: serde_json::Value = octocrab.graphql(&payload).await?;
            let refs = &response["data"]["repository"]["refs"];
            license = response["data"]["repository"]["licenseInfo"]
                .as_object()
                .and_then(|license| {
                    serde_json::from_value::<LicenseInfo>(serde_json::Value::Object(
                        license.clone(),
                    ))
                    .ok()
                });

            if let Some(nodes) = refs["nodes"].as_array() {
                for branch in nodes {
                    if let Some(name) = branch.get("name").and_then(|v| v.as_str()) {
                        if let Some(re) = &branch_filter {
                            if !re.is_match(name) {
                                continue;
                            }
                        }
                        let date_str = branch
                            .pointer("/target/committedDate")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let date = date_str.parse::<DateTime<Utc>>();
                        match date {
                            Ok(d) => {
                                branches.insert(name.to_string(), d);
                            }
                            Err(e) => {
                                eprintln!(
                                    "Failed to parse date for {owner}/{repo} branch {name}: {e}"
                                );
                            }
                        }
                    }
                }
            }
            if let Some(protection) =
                response["data"]["repository"]["branchProtectionRules"]["nodes"].as_array()
            {
                for rule in protection {
                    let parsed_rule: Option<BranchProtectionRule> =
                        serde_json::from_value(rule.clone()).ok();
                    if let Some(r) = parsed_rule {
                        protection_rules.push(r);
                    }
                }
            }

            let page_info = &refs["pageInfo"];
            let has_next_page = page_info["hasNextPage"].as_bool().unwrap_or(false);
            after = page_info["endCursor"].as_str().map(|s| s.to_string());

            if !has_next_page {
                break;
            }
        }

        let now = Utc::now();

        let mut old_branches: Vec<_> = branches
            .into_iter()
            .filter(|(_, age)| (now - age).num_days() >= days_ago)
            .filter(|(branch, _)| !blacklist.contains(branch))
            .collect();
        old_branches.sort_by_key(|(_, age)| *age);

        let old_branches: Vec<(String, String)> = old_branches
            .into_iter()
            .map(|(branch, age)| (branch, age.format("%Y-%m-%d").to_string()))
            .collect();
        println!("Protection rules: {protection_rules:#?}");

        Ok((
            old_branches,
            protection_rules,
            license,
            format!("{owner}/{repo}"),
        ))
    }

    /// Get the number of api calls left at the moment. Generally, the maximum number is 5000 in
    /// one hour.
    pub async fn get_rate_limit(&self) -> Result<(), GitError> {
        let response = async_retry!(
            ms = 100,
            timeout = 500,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = { self.octocrab.clone().ratelimit().get().await },
        );
        match response {
            Ok(rate) => {
                let rate = rate.rate;
                let (remaining, limit) = (rate.remaining, rate.limit);

                let reset_time = Utc
                    .timestamp_opt(rate.reset.try_into().unwrap(), 0)
                    .unwrap();
                let local_time = reset_time.with_timezone(&Local).format("%H:%M %Y-%m-%d");

                let time_zone = iana_time_zone::get_timezone()?;
                println!(
                    "REST API Rate limit: {remaining}/{limit} remaining. Resets at {local_time} ({time_zone})"
                );
                Ok(())
            }
            Err(e) => Err(GitError::GithubApiError(e)),
        }
    }
    /// Get the number of graphql api calls left at the moment. Generally, the maximum number is
    /// 5000. This is tracked separately from the rest api limits.
    pub async fn get_graphql_limit(&self) -> Result<(), GitError> {
        let query = r#"
            {
                rateLimit {
                    limit
                    remaining
                    resetAt
                }
            }"#;
        let payload = serde_json::json!({ "query": query });

        let response: Result<serde_json::Value, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 500,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = { self.octocrab.clone().graphql(&payload).await },
        );

        match response {
            Ok(resp) => {
                let rate_limit = &resp["data"]["rateLimit"];
                let limit = rate_limit["limit"].as_u64().unwrap_or(0);
                let remaining = rate_limit["remaining"].as_u64().unwrap_or(0);
                let reset_at_str = rate_limit["resetAt"].as_str().unwrap_or("");

                let reset_at = reset_at_str.parse::<DateTime<Utc>>()?;
                let local_time = reset_at.with_timezone(&Local).format("%H:%M %Y-%m-%d");
                let time_zone = iana_time_zone::get_timezone()?;

                println!(
                    "GraphQL API Rate limit: {remaining}/{limit} remaining. Resets at {local_time} ({time_zone})"
                );
                Ok(())
            }
            Err(e) => Err(GitError::GithubApiError(e)),
        }
    }
    fn println(&self, text: &str, pb: &Option<ProgressBar>) {
        match pb {
            Some(pb) => {
                if console::user_attended() {
                    pb.println(text);
                } else {
                    // Fallback if no TTY
                    println!("{text}");
                }
            }
            None => {
                println!("{text}");
            }
        }
    }
    fn eprintln(&self, text: &str, pb: &Option<ProgressBar>) {
        match pb {
            Some(pb) => {
                if console::user_attended() {
                    pb.println(text);
                } else {
                    // Fallback if no TTY
                    eprintln!("{text}");
                }
            }
            None => {
                eprintln!("{text}");
            }
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

fn create_progress_bar<T: Into<u64>>(len: T) -> Option<ProgressBar> {
    if console::user_attended() {
        return None;
    }
    let pb = ProgressBar::new(len.into());
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.magenta} [{wide_bar:.cyan/dark_blue}] [{elapsed}] {msg}",
        )
        .unwrap()
        .progress_chars("#>-"),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    Some(pb)
}
