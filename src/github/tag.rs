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
use std::collections::HashMap;
use std::error::Error;
use std::process::Command;
use temp_dir::TempDir;

use indexmap::IndexSet;
use serde_json::json;
use std::fmt::Write as _;

use crate::github::client::GithubClient;

#[derive(Deserialize)]
pub struct RepoResponse {
    pub data: RepoData,
}
#[derive(Deserialize)]
pub struct RepoData {
    pub repository: Repository,
}
#[derive(Deserialize)]
pub struct Repository {
    pub parent: Option<ParentRepo>,
    pub refs: Refs,
}
#[derive(Deserialize)]
pub struct ParentRepo {
    pub url: String,
}
#[derive(Deserialize)]
pub struct Refs {
    pub nodes: Vec<TagNode>,
    #[serde(rename = "pageInfo")]
    pub page_info: PageInfo,
}
#[derive(Deserialize)]
pub struct PageInfo {
    #[serde(rename = "hasNextPage")]
    pub has_next_page: bool,
    #[serde(rename = "endCursor")]
    pub end_cursor: Option<String>,
}
#[derive(Deserialize)]
pub struct TagNode {
    pub name: String,
    pub target: TagTarget,
}
#[derive(Deserialize)]
pub struct TagTarget {
    #[serde(rename = "__typename")]
    pub typename: String,
    pub oid: String,
    pub message: Option<String>,
    pub tagger: Option<Tagger>,
}
#[derive(Deserialize)]
pub struct Tagger {
    pub name: Option<String>,
    pub email: Option<String>,
    pub date: Option<String>,
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
    pub async fn get_tags(&self, url: &str) -> Result<IndexSet<TagInfo>, GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);

        let mut all_tags: IndexSet<TagInfo> = IndexSet::new();
        let mut has_next_page = true;
        let mut after: Option<String> = None;
        let per_page = 100;

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

            let mut repo = res.data.repository;
            let mut parent_url = repo.parent.as_ref().map(|p| p.url.clone());

            for tag in &mut repo.refs.nodes {
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

                all_tags.insert(TagInfo {
                    name: std::mem::take(&mut tag.name),
                    tag_type,
                    sha: std::mem::take(&mut tag.target.oid),
                    message,
                    tagger_name,
                    tagger_email,
                    tagger_date,
                    parent_url: std::mem::take(&mut parent_url),
                });
            }
            has_next_page = repo.refs.page_info.has_next_page;
            after = repo.refs.page_info.end_cursor;
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
        let missing = missing_in_fork.len();

        if missing > 0 {
            let mut slack_message = String::new();
            let missing_annotated: IndexSet<TagInfo> = missing_in_fork
                .iter()
                .filter(|t| t.tag_type == TagType::Annotated)
                .cloned()
                .collect();

            if missing == 1 {
                let _ = writeln!(
                    slack_message,
                    "*:information_source: *Missing 1 tag in {url}*:"
                );
            } else {
                let _ = writeln!(
                    slack_message,
                    ":information_source: *Missing {missing} tags in {url}*:"
                );
            }

            if !missing_annotated.is_empty() {
                slack_message.push_str("The following annotated tags are missing:\n");
                for tag in &missing_annotated {
                    let _ = writeln!(slack_message, ">• `{}`", tag.name);
                }
            }
            self.append_slack_message(slack_message).await;
        } else {
            println!("Tags are up to date for {url}");
            self.append_slack_message(format!(":white_check_mark: Tags are up to date for {url}"))
                .await;
        }

        let compare = Comparison {
            fork_tags,
            parent_tags,
            missing_in_fork,
        };
        Ok(compare)
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

        let mut errors: Vec<(String, GitError)> = Vec::new();
        handle_futures_unordered!(
            repositories.into_iter().flatten().map(|repo|{
                let url = repo.url.clone();
                let owner = repo.owner.clone();
                let repo_name = repo.repo_name.clone();
                println!("   Processing {owner}/{repo_name}");
                (owner, repo_name, url )
            }),
            |owner, repo, url| self.diff_tags(&url).map(|result| (owner, repo, result)),
            (owner, repo, result) {
                match result {
                    Ok(r) => {
                        diffs.insert(repo.to_string(), r);
                        println!("✅ Successfully diffed tags for {owner}/{repo}");
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
    pub async fn sync_tags(&self, url: &str, process_annotated: bool) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let parent = self.get_parent_repo(url).await?;
        let (owner, repo) = (info.owner, info.repo_name);
        let missing = self.compare_tags(url, &parent).await?.missing_in_fork;
        if missing.is_empty() {
            println!("No missing tags in {url}");
            return Ok(());
        }

        // Split `missing` into two different `IndexSet`, based on their type of tag
        let (lightweight, annotated): (IndexSet<TagInfo>, IndexSet<TagInfo>) = missing
            .into_iter()
            .partition(|t| t.tag_type == TagType::Lightweight);

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
                        Ok(()) => println!("Successfully synced tag '{name}' in '{repo}'"),
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
            let (annotated, lightweight) = tokio::join!(
                self.sync_annotated_tags(&annotated, &ssh_url),
                lightweight_fut,
            );
            let tag_results = [("annotated", annotated), ("lightweight", lightweight)];

            for (tag_type, result) in tag_results {
                if let Err(e) = result {
                    eprintln!("Failed to sync {tag_type} tags for {owner}/{repo}: {e}");
                    self.append_slack_error(format!(
                        "❌ Failed to sync {tag_type} tags for {owner}/{repo}: {e}",
                    ))
                    .await;
                }
            }
        // If we're only processing lightweight tags, skip all of the above
        } else {
            lightweight_fut.await?;
        }

        self.append_slack_message(format!("✅ Successfully synced tags for {owner}/{repo}"))
            .await;
        Ok(())
    }
    /// Sync tags for all configured repositories
    pub async fn sync_all_tags(
        &self,
        process_annotated: bool,
        repositories: Vec<String>,
    ) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for url in repositories {
            futures.push(async move {
                let result = self.sync_tags(&url, process_annotated).await;
                (url, result)
            });
        }
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(()) => {
                    self.append_slack_message(format!("✅ Successfully synced tags for {repo}"))
                        .await;
                    println!("✅ Successfully synced tags for {repo}");
                }
                Err(e) => {
                    self.append_slack_error(format!("❌ Failed to sync tags for {repo}: {e}"))
                        .await;
                    eprintln!("❌ Failed to sync tags for {repo}");
                    errors.push((repo, e));
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
        owner: &str,
        repo: &str,
        tag: &TagInfo,
    ) -> Result<(), GitError> {
        let body = json!({
            "ref": format!("refs/tags/{}", tag.name),
            "sha": tag.sha,
        });
        let _lock = self.semaphore.clone().acquire_owned().await?;

        let response: Result<serde_json::Value, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                self.octocrab
                    .clone()
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
    ///
    /// We use `tokio::task::spawn_blocking` to make sure we don't make any other async functions
    /// hang. `Command::new` does block.
    pub async fn sync_annotated_tags(
        &self,
        tags: &IndexSet<TagInfo>,
        ssh_url: &str,
    ) -> Result<(), GitError> {
        if tags.is_empty() {
            return Ok(());
        }
        let _semaphore_lock = self.semaphore.clone().acquire_owned().await?;
        let tags = tags.clone();
        let ssh_url = ssh_url.to_string();
        tokio::task::spawn_blocking(move || {
            let first_tag = tags.first().unwrap();

            let Some(parent_url) = &first_tag.parent_url else {
                return Err(GitError::NoUpstreamRepo);
            };

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
            for tag in &tags {
                fetch_args.push("tag");
                fetch_args.push(tag.name.as_str());
            }

            Command::new("git").args(&fetch_args).status()?;

            // Only push the newly added annotated tags
            let mut push_args = vec!["-C", tmp_str, "push", "origin"];
            push_args.extend(tags.iter().map(|tag| tag.name.as_str()));

            Command::new("git").args(&push_args).status()?;
            Ok(())
        })
        .await?
    }

    /// Create a tag for a specific repository
    pub async fn create_tag(&self, url: &str, tag: &str, branch: &str) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let sha = self.get_branch_sha(url, branch).await?;
        let (owner, repo) = (info.owner, info.repo_name);
        // Acquire a lock on the semaphore
        let _permit = self.semaphore.clone().acquire_owned().await?;
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
                let tag = tag.to_string();
                let repo = repo.to_string();

                let message =
                    format!(":white_check_mark: Successfully created tag '{tag}' for {repo}");

                self.append_slack_message(message).await;
                println!("Successfully created tag '{tag}' for {repo}");
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
                Ok(()) => println!("Successfully created tag '{tag}' for '{repo}'"),
                Err(e) => eprintln!("Failed to create tag '{tag}' for '{repo}': {e}"),
            }
        }
        Ok(())
    }

    /// Delete the specified tag for a repository. Deleting a tag does not necessarily return a
    /// json response, so we handle this one differently
    pub async fn delete_tag(&self, url: &str, tag: &str) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);

        // Acquire a lock on the semaphore
        let _permit = self.semaphore.clone().acquire_owned().await?;

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
                Ok(()) => println!("Successfully deleted tag '{tag}' for {repo}"),
                Err(e) => eprintln!("Failed to delete tag '{tag}' for {repo}: {e}"),
            }
        }
        Ok(())
    }
}
