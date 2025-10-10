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

use crate::error::{GitError, is_retryable};
use crate::github::client::Comparison;
use crate::utils::repo::{RepoInfo, TagInfo, TagType, get_repo_info_from_url, http_to_ssh_repo};
use crate::{async_retry, handle_api_response, handle_futures_unordered};
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt, future::try_join};
use octocrab::models::repos::Ref;
use octocrab::params::repos::Reference;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::hash::Hash;
use std::process::Command;
use temp_dir::TempDir;

use indexmap::IndexSet;
use serde_json::json;
use std::fmt::{Display, Write as _};

use crate::github::client::GithubClient;

/// The root level response from github
#[derive(Deserialize)]
pub struct RepoResponse {
    pub data: RepoData,
}
/// Struct to deserialize the repository data into
#[derive(Deserialize)]
pub struct RepoData {
    pub repository: Repository,
}
/// An actual repository containing its parent if it has one
#[derive(Deserialize)]
pub struct Repository {
    pub parent: Option<ParentRepo>,
    pub refs: Refs,
}
/// A parent repository
#[derive(Deserialize)]
pub struct ParentRepo {
    pub url: String,
}
/// The refs in a repository
#[derive(Deserialize)]
pub struct Refs {
    pub nodes: Vec<TagNode>,
    #[serde(rename = "pageInfo")]
    pub page_info: PageInfo,
}
/// Information about pagination needed for queries
#[derive(Deserialize)]
pub struct PageInfo {
    #[serde(rename = "hasNextPage")]
    pub has_next_page: bool,
    #[serde(rename = "endCursor")]
    pub end_cursor: Option<String>,
}
/// The actual tag node
#[derive(Deserialize, Debug)]
pub struct TagNode {
    pub name: String,
    pub target: TagTarget,
}
/// Information about the tag, particularly it's type (annotated or lightweight)
#[derive(Deserialize, Debug)]
pub struct TagTarget {
    #[serde(rename = "__typename")]
    pub typename: String,
    pub oid: String,
    pub target: Option<Box<TagTarget>>,
}

impl GithubClient {
    /// This can be used to fetch tags in a more api-call efficient way than using the rest api.
    /// It does mean we have to manually query the graphql endpoint and manually parse the json
    /// output, rather than having it done for us by octocrab.
    /// We can't actually get all the information required for an annotated tag, but we can use it
    /// to distinguish between lightweight and annotated tags. If we don't get any annotated tags,
    /// we can skip the fairly slow git clone and push process
    ///
    /// `IndexSet` is an implementation of an orderered Set.
    pub async fn get_tags<T: AsRef<str>>(
        &self,
        url: T,
    ) -> Result<(IndexSet<TagInfo>, HashSet<String>), GitError> {
        let info = get_repo_info_from_url(&url)?;
        let (owner, repo) = (info.owner, info.repo_name);

        let mut all_tags: IndexSet<TagInfo> = IndexSet::new();
        let mut has_next_page = true;
        let mut after: Option<String> = None;
        let per_page = 100;

        let mut parent_urls: HashSet<String> = HashSet::new();

        let octocrab = self.octocrab.clone();
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
                                target {
                                    __typename
                                    oid
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
        let url_owned = url.as_ref().to_string();
        while has_next_page {
            // Acquire a lock on the semaphore
            let permit = self.semaphore.clone().acquire_owned().await?;

            let payload = json!({
                "query": query,
                "variables": {
                    "owner": owner,
                    "repo": repo,
                    "first": per_page,
                    "after": after,
                }
            });
            let res: RepoResponse = async_retry!(
                ms = 100,
                timeout = 5000,
                retries = 3,
                error_predicate = |e: &octocrab::Error| is_retryable(e),
                body = { octocrab.graphql(&payload).await },
            )?;

            // Drop the lock on the semaphore so other network activities can potentially run
            drop(permit);

            let repo = res.data.repository;

            let parent_url = repo.parent.as_ref().map(|p| p.url.clone());

            for tag in repo.refs.nodes {
                let tag_type = match tag.target.typename.as_str() {
                    "Tag" => TagType::Annotated,
                    "Commit" => TagType::Lightweight,
                    other => return Err(GitError::Other(format!("Unknown tag type '{other}'"))),
                };
                let sha = tag.target.oid;
                let commit_sha = tag.target.target.map(|inner| inner.oid);

                if let Some(url) = parent_url.as_ref()
                    && !parent_urls.contains(&url_owned)
                {
                    parent_urls.insert(url.to_owned());
                }

                all_tags.insert(TagInfo {
                    name: tag.name,
                    tag_type,
                    sha,
                    url: url_owned.clone(),
                    commit_sha,
                });
            }
            has_next_page = repo.refs.page_info.has_next_page;
            after = repo.refs.page_info.end_cursor;
        }

        Ok((all_tags, parent_urls))
    }
    pub async fn compare_tags<T: AsRef<str> + Display + Copy>(
        &self,
        url: T,
        parent: &RepoInfo,
    ) -> Result<Comparison, GitError> {
        let ((fork_tags, mut fork_parents), (parent_tags, upstream_parents)) =
            try_join(self.get_tags(url), self.get_tags(&parent.url)).await?;
        if self.is_tty {
            println!(
                "Fork tags: {}\nParent tags: {}",
                fork_tags.len(),
                parent_tags.len()
            );
        }

        let mut missing_annotated: IndexSet<TagInfo> = IndexSet::new();
        let mut missing_in_fork: IndexSet<TagInfo> = IndexSet::new();

        for tag in parent_tags.difference(&fork_tags) {
            if tag.tag_type == TagType::Annotated {
                missing_annotated.insert(tag.clone());
            }
            missing_in_fork.insert(tag.clone());
        }

        let missing = missing_in_fork.len();

        if missing > 0 {
            let mut slack_message = String::with_capacity(256);
            if missing == 1 {
                let _ = writeln!(
                    slack_message,
                    ":information_source: Missing 1 tag in {url}:"
                );
            } else {
                let _ = writeln!(
                    slack_message,
                    ":information_source: Missing {missing} tags in {url}:"
                );
            }

            let total_annotated = missing_annotated.len();
            let max_tags_display = std::cmp::min(10, total_annotated);
            if total_annotated > max_tags_display {
                writeln!(
                    slack_message,
                    "The following annotated tags are missing, as well as {} others",
                    total_annotated - max_tags_display
                )
                .unwrap();
            } else if total_annotated > 0 {
                slack_message.push_str("The following annotated tags are missing:\n");
            }

            for tag in &missing_annotated[0..max_tags_display] {
                let _ = writeln!(slack_message, ">• `{}`", tag.name);
            }
        } else if self.is_tty {
            println!("Tags are up to date for {url}");
        }
        fork_parents.extend(upstream_parents);

        let compare = Comparison {
            missing_in_fork,
            parent_urls: fork_parents,
        };
        Ok(compare)
    }
    /// Get a diff of tags between a single forked repository and its parent repository.
    pub async fn diff_tags(&self, url: &str) -> Result<Comparison, GitError> {
        let parent = self.get_parent_repo(url).await?;
        let comparison = self.compare_tags(url, &parent).await?;

        Ok(comparison)
    }
    /// Get a diff of all configured repositories tags, compared against their parent.
    pub async fn diff_all_tags(&self, repositories: Vec<String>) -> Result<(), GitError> {
        let repositories: Vec<Result<RepoInfo, _>> =
            repositories.iter().map(get_repo_info_from_url).collect();

        let mut diffs: HashMap<String, Comparison> = HashMap::new();

        let mut errors: Vec<(String, GitError)> = Vec::new();
        handle_futures_unordered!(
            repositories.into_iter().flatten().map(|repo|{
                let url = repo.url.clone();
                let owner = repo.owner.clone();
                let repo_name = repo.repo_name.clone();
                if self.is_tty {
                    println!("   Processing {owner}/{repo_name}");
                }
                (owner, repo_name, url )
            }),
            |owner, repo, url| self.diff_tags(&url).map(|result| (owner, repo, result)),
            (owner, repo, result) {
                match result {
                    Ok(r) => {
                        diffs.insert(repo.to_string(), r);
                        if self.is_tty {
                            println!("✅ Successfully diffed tags for {owner}/{repo}");
                        } else {
                            println!("{owner}/{repo}");
                        }
                    },
                    Err(e) => {
                        eprintln!("❌ Failed to diff tags for {owner}/{repo}: {e}");
                        errors.push((format!("{owner}/{repo}"), e));
                    }
                }
            }
        );

        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(())
    }

    /// Sync many tags asynchronously. We can use the github api to sync lightweight tags, but to
    /// sync annotated tags we need to use git. Unfortunately there's no way around that.
    pub async fn sync_tags<T: AsRef<str> + Display + Copy>(
        &self,
        url: T,
        parent_url: Option<T>,
        process_annotated: bool,
    ) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;

        let parent = if let Some(u) = parent_url {
            let info = get_repo_info_from_url(u)?;
            RepoInfo {
                repo_name: info.repo_name,
                owner: info.owner,
                url: u.to_string(),
                main_branch: info.main_branch,
            }
        } else {
            self.get_parent_repo(url).await?
        };
        let (owner, repo) = (info.owner, info.repo_name);
        let all_tags = self.compare_tags(url, &parent).await?;

        let parent_urls = all_tags.parent_urls;

        let missing = all_tags.missing_in_fork;
        let num_tags_missing = missing.len();

        if missing.is_empty() {
            if self.is_tty {
                println!("No missing tags in {url}");
            }
            return Ok(());
        }

        let mut errors: Vec<(String, GitError)> = Vec::with_capacity(missing.len());

        // Split `missing` into two different `IndexSet`, based on their type of tag
        let (lightweight, annotated): (IndexSet<TagInfo>, IndexSet<TagInfo>) = missing
            .into_iter()
            .partition(|t| t.tag_type == TagType::Lightweight);
        let lightweight_fut = async {
            handle_futures_unordered!(
                lightweight.iter().map(|tag| {
                    let owner = owner.clone();
                    let repo = repo.clone();
                    let name = tag.name.clone();
                    (repo, name, owner, tag)
                }),
                |repo, name, owner, tag| self.sync_lightweight_tag(&owner, &repo.clone(), tag).map(|result|(repo, name, result)),
                (repo, name, result) {
                    match result {
                        Ok(()) => {
                            if self.is_tty {
                                println!("Successfully synced tag '{name}' in '{repo}'");
                            }
                        },
                        Err(e) => {
                            eprintln!("Failed to sync '{name}' for '{repo}': {e}");
                            errors.push((name, e));
                        }
                    }
                }
            );
            Ok::<(), GitError>(())
        };
        // Run both of these at the same time. Annotated tags are much slower to sync than
        // ligthweight tags since we need to clone, fetch from upstream, then push.
        if process_annotated {
            let ssh_url = http_to_ssh_repo(url)?;
            let (annotated, lightweight) = tokio::join!(
                self.sync_annotated_tags(&annotated, &ssh_url, parent_urls),
                lightweight_fut,
            );
            let tag_results = [("annotated", annotated), ("lightweight", lightweight)];

            for (tag_type, result) in tag_results {
                if let Err(e) = result {
                    eprintln!("Failed to sync {tag_type} tags for {owner}/{repo}: {e}");
                    errors.push((tag_type.to_string(), e));
                }
            }
        // If we're only processing lightweight tags, skip all of the above
        } else {
            let result = lightweight_fut.await;
            if let Err(e) = result {
                self.append_slack_error(format!(
                    "Failed to sync lightweight tags for {owner}/{repo}: {e}"
                ))
                .await;
                return Err(GitError::SyncFailure {
                    ref_type: String::from("lightweight tags"),
                    repository: format!("{owner}/{repo}"),
                });
            }
        }
        if !errors.is_empty() {
            self.append_slack_error(format!(
                "Encountered {} errors while trying to sync tags for {owner}/{repo}. Some tags may have already synced.",
                errors.len()
            ))
            .await;
            return Err(GitError::MultipleErrors(errors));
        }
        let mut message =
            format!(":white_check_mark: Synced {num_tags_missing} tags for {owner}/{repo}\n");
        if process_annotated && !annotated.is_empty() && errors.is_empty() {
            let num_annotated = annotated.len();
            if num_annotated == 1 {
                let _ = writeln!(message, "Of which {num_annotated} is an annotated tag:");
            } else {
                let _ = writeln!(message, "Of which {num_annotated} are annotated tags:",);
            }
            for (i, tag) in annotated.iter().enumerate() {
                if i >= 10 {
                    let _ = writeln!(message, "\t and {} others", num_annotated - i);
                    break;
                }
                let _ = write!(message, "\t• `{}`", tag.name);
            }
        }
        if !lightweight.is_empty() && errors.is_empty() {
            let num_lightweight = lightweight.len();
            if annotated.is_empty() {
                let _ = writeln!(message, "Of which {num_lightweight} are lightweight tags:");
            } else if annotated.len() == 1 {
                let _ = writeln!(message, "\nAnd {num_lightweight} is a lightweight tag:");
            } else {
                let _ = writeln!(message, "\nAnd {num_lightweight} are lightweight tags:");
            }
            for (i, tag) in lightweight.iter().enumerate() {
                if i >= 10 {
                    let _ = writeln!(message, "\t and {} others", num_lightweight - i);
                    break;
                }
                let _ = write!(message, "\t• `{}`", tag.name);
            }
        }

        self.append_slack_message(message).await;
        Ok(())
    }
    /// Sync tags for all configured repositories
    pub async fn sync_all_tags<T: AsRef<str> + ToString + Display + Eq + Hash>(
        &self,
        process_annotated: bool,
        repositories: &[T],
        fork_workaround: HashMap<T, T>,
    ) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for url in repositories {
            let parent_url = fork_workaround.get(url);
            futures.push(async move {
                let result = self.sync_tags(url, parent_url, process_annotated).await;
                (url, result)
            });
        }
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(()) => {
                    if self.is_tty {
                        println!("✅ Successfully synced tags for {repo}");
                    }
                }
                Err(e) => {
                    self.append_slack_error(format!("❌ Failed to sync tags for {repo}: {e}"))
                        .await;
                    eprintln!("❌ Failed to sync tags for {repo}");
                    errors.push((repo.to_string(), e));
                }
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }

        Ok(())
    }
    /// Sync lightweight tags from the parent repo to the forked repo. This can be trivially done
    /// using the github api, so we don't need to call out to
    pub async fn sync_lightweight_tag(
        &self,
        owner: impl AsRef<str> + Display,
        repo: impl AsRef<str> + Display,
        tag: &TagInfo,
    ) -> Result<(), GitError> {
        let body = json!({
            "ref": format!("refs/tags/{}", tag.name),
            "sha": tag.sha,
        });
        let octocrab = self.octocrab.clone();

        let response: Result<serde_json::Value, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let _lock = self.semaphore.clone().acquire_owned().await;
                octocrab
                    .post::<serde_json::Value, _>(
                        format!("/repos/{owner}/{repo}/git/refs"),
                        Some(&body),
                    )
                    .await
            },
        );

        handle_api_response!(
            response,
            format!("Unable to sync tag {} in {owner}/{repo}", tag.name),
            |_| {
                if self.is_tty {
                    println!(
                        "Successfully synced lightweight tag '{}' {owner}/{repo}",
                        tag.name
                    );
                }
                Ok::<(), GitError>(())
            },
        )?;
        Ok(())
    }

    /// Sync all new annotated tags from a forked repo with its parent.
    /// Doing this *requires* using git (or some re-implementation of git). Syncing annotated tags
    /// through the github api with all of its fields, including signing, is currently not
    /// possible.
    ///
    /// We use `tokio::task::spawn_blocking` to make sure we don't make any other async functions
    /// hang. `Command::new` can end up blocking other threads.
    pub async fn sync_annotated_tags(
        &self,
        tags: &IndexSet<TagInfo>,
        ssh_url: impl AsRef<str> + ToString,
        parent_urls: HashSet<String>,
    ) -> Result<(), GitError> {
        if tags.is_empty() {
            return Ok(());
        }
        let lock = self.semaphore.clone().acquire_owned().await?;
        let tags = tags.clone();
        let tag_len = tags.len();
        let ssh_url = ssh_url.to_string();
        let output_url = ssh_url.to_string();
        let result = tokio::task::spawn_blocking(move || {
            let _lock = lock;
            let parent_urls: Vec<String> = parent_urls.into_iter().collect();

            // Use a temp directory for the git repository so it's cleaned up automatically
            let tmp_dir = TempDir::new()
                .map_err(|e| GitError::Other(format!("Failed to create temp dir: {e}")))?;
            let tmp = tmp_dir.path();
            let tmp_str = tmp
                .to_str()
                .ok_or_else(|| GitError::Other("Temp dir not valid UTF-8".to_string()))?;

            // Clone with the bare minimum information to reduce the amount we download
            Command::new("git")
                .args([
                    "clone",
                    "--bare",
                    "--filter=blob:none",
                    "--depth=1",
                    &ssh_url,
                    tmp_str,
                ])
                .status()?;

            for (i, upstream_url) in parent_urls.iter().enumerate() {
                // Add any other parent urls as remotes, in case the fork has multiple parents
                Command::new("git")
                    .args([
                        "-C",
                        tmp_str,
                        "remote",
                        "add",
                        &format!("upstream{i}"),
                        upstream_url,
                    ])
                    .status()?;
            }
            for (i, upstream_url) in parent_urls.iter().enumerate() {
                let url = format!("upstream{i}");
                let output = Command::new("git")
                    .args(["-C", tmp_str, "remote", "get-url", &url])
                    .output()?;
                if output.status.success() {
                    Command::new("git")
                        .args(["-C", tmp_str, "remote", "set-url", &url, upstream_url])
                        .status()?;
                } else {
                    Command::new("git")
                        .args(["-C", tmp_str, "remote", "add", &url, upstream_url])
                        .status()?;
                }
            }
            let mut branches_to_add_update:Vec<String> = Vec::new();
            for tag in &tags {
                let fetch_status = Command::new("git")
                    .args([
                        "-C",
                        tmp_str,
                        "fetch",
                        &tag.url,
                        &format!("refs/tags/{}:refs/tags/{}", tag.name, tag.name),
                    ])
                    .status()?;
                if !fetch_status.success() {
                    return Err(GitError::NoSuchTag(
                        tag.name.to_string(),
                    ));
                }
                if let Some(sha) = tag.commit_sha.as_ref() {
                    let output = Command::new("git")
                        .args([
                            "-C",
                            tmp_str,
                            "-r",
                            "contains",
                            sha,
                        ])
                        .output()?;
                    if !output.status.success() {
                        eprintln!("Commit {sha} does not exist in any configured remote.");
                        return Err(GitError::NoSuchReference(sha.to_string()));
                    }
                    let branch_output = String::from_utf8_lossy(&output.stdout);
                    if !branch_output.contains("origin") {
                        for line in branch_output.lines() {
                            branches_to_add_update.push(line.trim().to_string());
                        }
                    }
                }
            }
            let mut slack_error = String::new();
            if !branches_to_add_update.is_empty() {
                slack_error = String::from("You are likely missing some commits for the new annotated tags.\nThe following branches may need to be added or updated:\n");
                for branch in branches_to_add_update {
                    // using write/writeln prevents allocating a new string every time we add to it, as compared to format!() 
                    let _ = writeln!(slack_error, "\t• {branch}");
                }
                eprint!("{slack_error}");
            }

            // Only push the newly added annotated tags
            let mut push_args = vec!["-C", tmp_str, "push", "origin"];
            push_args.extend(tags.iter().map(|tag| tag.name.as_str()));

            let status = Command::new("git").args(&push_args).status()?;
            if !status.success() {
                              return Err(GitError::GitPushError(ssh_url.to_string()
                ));
            }
            if slack_error.is_empty() {
                Ok(None)
            } else {
                Ok(Some(slack_error))
            }
        })
        .await?;
        match result {
            Ok(Some(err)) => {
                self.append_slack_error(err).await;
            }
            Ok(None) => {
                if self.is_tty {
                    println!("Successfully synced {tag_len} annotated tags in {output_url}");
                }
            }
            Err(e) => {
                self.append_slack_error(format!(
                    ":x: Failed to sync annotated tags in {output_url}: {e}"
                ))
                .await;
                return Err(GitError::Other(format!(
                    "Failed to sync annotated tags in {output_url}: {e}"
                )));
            }
        }
        Ok(())
    }

    /// Create a tag for a specific repository
    pub async fn create_tag(
        &self,
        url: impl AsRef<str> + Copy,
        tag: impl AsRef<str> + ToString + Display,
        branch: impl AsRef<str> + Display,
    ) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let sha = self.get_branch_sha(url, branch).await?;
        let (owner, repo) = (info.owner, info.repo_name);
        let octocrab = self.octocrab.clone();
        let res: Result<Ref, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let _permit = self.semaphore.clone().acquire_owned().await;
                octocrab
                    .repos(&owner, &repo)
                    .create_ref(&Reference::Tag(tag.to_string()), sha.clone())
                    .await
            },
        );

        match res {
            Ok(_) => {
                let tag = tag.to_string();
                let repo = repo.to_string();

                let message =
                    format!(":white_check_mark: Successfully created tag '{tag}' for {repo}");

                self.append_slack_message(message).await;
                if self.is_tty {
                    println!("Successfully created tag '{tag}' for {repo}");
                }
                Ok(())
            }
            Err(e) => {
                let a = e.source().unwrap();
                self.append_slack_error(format!(":x: Failed to create '{tag}' for {repo}: {a}"))
                    .await;
                Err(GitError::GithubApiError(e))
            }
        }
    }

    /// Create the tag for all configured repositories
    pub async fn create_all_tags<
        T: AsRef<str> + ToString + Display + Copy,
        U: AsRef<str> + ToString + Display + Copy,
        V: AsRef<str> + ToString + Display,
    >(
        &self,
        tag: T,
        branch: U,
        repositories: &[V],
    ) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            futures.push(async move {
                let result = self.create_tag(&repo, tag, branch).await;
                (repo, result)
            });
        }

        let mut errors: Vec<(String, GitError)> = Vec::new();

        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("Failed to create tag '{tag}' for '{repo}': {e}");
                    errors.push((repo.to_string(), e));
                }
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(())
    }

    /// Delete the specified tag for a repository. Deleting a tag does not necessarily return a
    /// json response, so we handle this one differently
    pub async fn delete_tag<
        T: AsRef<str> + ToString + Display,
        U: AsRef<str> + ToString + Display,
    >(
        &self,
        url: T,
        tag: U,
    ) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);

        // Acquire a lock on the semaphore
        let permit = self.semaphore.clone().acquire_owned().await?;
        // TODO: Wrap in async_retry! macro
        let response = self
            .octocrab
            .clone()
            ._delete(
                format!("/repos/{owner}/{repo}/git/refs/tags/{tag}"),
                None::<&()>,
            )
            .await;
        drop(permit);

        match response {
            Ok(resp) => {
                if resp.status().is_success() {
                    if self.is_tty {
                        println!("Successfully deleted tag '{tag}' for {repo}");
                    }
                    self.append_slack_message(format!("Tag '{tag}' has been deleted from {repo}"))
                        .await;
                    Ok(())
                } else {
                    let status_code = resp.status().as_u16();

                    match status_code {
                        422 => {
                            if self.is_tty {
                                println!(
                                    "Tag '{tag}' does not exist in {repo}. Nothing to delete."
                                );
                            }
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
    pub async fn delete_all_tags<
        T: AsRef<str> + ToString + Display + Copy,
        U: AsRef<str> + ToString + Display,
    >(
        &self,
        tag: T,
        repositories: &[U],
    ) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            futures.push(async move {
                let result = self.delete_tag(repo, tag).await;
                (repo, result)
            });
        }
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("Failed to delete tag '{tag}' for {repo}: {e}");
                    errors.push((repo.to_string(), e));
                }
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(())
    }
}
