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
use crate::github::client::GithubClient;
use crate::utils::repo::get_repo_info_from_url;
use crate::{async_retry, handle_api_response};
use chrono::DateTime;
use futures::{StreamExt, stream::FuturesUnordered};
use octocrab::params::repos::Reference;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Write};
use std::sync::Arc;

/// Graphql query to fetch branches and their commit dates
static GRAPHQL_QUERY: &str = r#"
            query ($owner: String!, $repo: String!, $after: String) {
                repository(owner: $owner, name: $repo) {
                    refs(refPrefix: "refs/heads/", first: 100, after: $after) {
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
            }"#;

#[derive(Debug, Deserialize)]
struct BranchNode {
    name: String,
    target: CommitTarget,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitTarget {
    committed_date: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BranchConnection {
    nodes: Vec<BranchNode>,
    page_info: PageInfo,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RepositoryData {
    repository: RepoRefs,
}

#[derive(Debug, Deserialize)]
struct RepoRefs {
    refs: BranchConnection,
}
#[derive(Debug, Deserialize)]
struct BranchCommits {
    data: RepositoryData,
}

impl GithubClient {
    /// Sync a single repository with its parent repository. Optionally, specify a branch to sync.
    /// You may need to do this if a new tag points to a commit in specific branch.
    pub async fn sync_fork<T: AsRef<str>>(
        &self,
        url: T,
        branch: Option<&String>,
    ) -> Result<(), GitError> {
        let info = get_repo_info_from_url(&url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        println!("Syncing {owner}/{repo} with its parent repository...");

        let parent = self.get_parent_repo(url).await?;
        let body = if let Some(branch) = branch {
            serde_json::json!({"branch": branch})
        } else {
            serde_json::json!({"branch": parent.main_branch})
        };
        println!("{body:?}");
        let octocrab = self.octocrab.clone();
        // Retry if a potentially recoverable error is detected
        let response: Result<serde_json::Value, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                octocrab
                    .post(format!("/repos/{owner}/{repo}/merge-upstream"), Some(&body))
                    .await
            },
        );
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
    /// Fetch all branches for a single repository
    async fn fetch_branches<T: AsRef<str> + Serialize, U: AsRef<str> + Serialize>(
        &self,
        owner: T,
        repository: U,
    ) -> Result<HashMap<String, String>, GitError> {
        let mut branches: HashMap<String, String> = HashMap::new();
        let mut has_next_page = true;
        let mut cursor: Option<String> = None;
        let octocrab = self.octocrab.clone();

        while has_next_page {
            let permit = Arc::clone(&self.semaphore).acquire_owned().await?;
            let payload = serde_json::json!({
                "query": GRAPHQL_QUERY,
                "variables": {
                    "owner": owner,
                    "repo": repository,
                    "after": cursor,
                },
            });

            let res: BranchCommits = async_retry!(
                ms = 100,
                timeout = 5000,
                retries = 3,
                error_predicate = |e: &octocrab::Error| is_retryable(e),
                body = { octocrab.graphql(&payload).await },
            )?;

            // Drop the lock on the semaphore so other network activities can potentially run
            drop(permit);

            res.data.repository.refs.nodes.iter().for_each(|node| {
                branches.insert(node.name.clone(), node.target.committed_date.clone());
            });

            has_next_page = res.data.repository.refs.page_info.has_next_page;
            cursor = res.data.repository.refs.page_info.end_cursor;
        }

        Ok(branches)
    }
    /// Go through and try to sync every branch that's common between both the fork and its parent.
    /// This operation takes longer than only syncing one branch
    pub async fn sync_fork_recursive<T: AsRef<str>>(&self, url: T) -> Result<(), GitError> {
        let info = get_repo_info_from_url(&url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        println!("Syncing {owner}/{repo} with its parent repository...");

        let parent = self.get_parent_repo(url).await?;
        let octocrab = self.octocrab.clone();

        let parent_branches = self.fetch_branches(&parent.owner, &repo).await?;
        let fork_branches = self.fetch_branches(&owner, &repo).await?;

        let common_branches: HashSet<_> = parent_branches
            .keys()
            .collect::<HashSet<_>>()
            .intersection(&fork_branches.keys().collect::<HashSet<_>>())
            .copied()
            .collect();

        let mut branches_to_sync: Vec<String> = Vec::new();

        for branch in common_branches {
            let parent_date_string = parent_branches.get(branch);
            let fork_date_string = fork_branches.get(branch);
            let (parent_date, fork_date) = match (parent_date_string, fork_date_string) {
                (Some(parent), Some(fork)) => (
                    DateTime::parse_from_rfc3339(parent),
                    DateTime::parse_from_rfc3339(fork),
                ),
                (None, _) | (_, None) => continue,
            };
            match (parent_date, fork_date) {
                (Ok(parent), Ok(fork)) => {
                    if parent > fork {
                        branches_to_sync.push(branch.to_string());
                    }
                }
                (Err(e), _) | (_, Err(e)) => {
                    return Err(GitError::DateParseError(e));
                }
            }
        }

        for branch in branches_to_sync {
            let body = serde_json::json!({"branch": branch});
            // Retry if a potentially recoverable error is detected
            let response: Result<serde_json::Value, octocrab::Error> = async_retry!(
                ms = 100,
                timeout = 5000,
                retries = 3,
                error_predicate = |e: &octocrab::Error| is_retryable(e),
                body = {
                    octocrab
                        .post(format!("/repos/{owner}/{repo}/merge-upstream"), Some(&body))
                        .await
                },
            );
            match response {
                Ok(_) => {
                    if self.is_tty {
                        println!(
                            "Successfully synced {owner}/{repo} Branch: {branch} with its parent repository"
                        );
                    }
                }
                Err(e) => {
                    let mut message = format!(
                        "Failed to sync {owner}/{repo} Branch: {branch} with its parent repository."
                    );
                    if let octocrab::Error::GitHub { source, .. } = &e {
                        let _ = write!(message, " GitHub API error: {}", source.message);
                    }
                    eprintln!("{message}");
                    self.append_slack_error(message).await;
                }
            }
        }

        Ok(())
    }

    /// Sync all configured repositories. Only repositories that have a parent repository
    /// should be passed to this function
    pub async fn sync_all_forks<T: AsRef<str> + Display>(
        &self,
        repositories: &[T],
        recursive: bool,
    ) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            let semaphore_permit = self.semaphore.clone().acquire_owned().await?;
            futures.push(async move {
                let result = if recursive {
                    self.sync_fork_recursive(&repo).await
                } else {
                    self.sync_fork(repo, None).await
                };
                drop(semaphore_permit);
                (repo, result)
            });
        }
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(()) => {
                    println!("✅ Successfully synced {repo}");
                }
                Err(e) => {
                    println!("❌ Failed to sync {repo}: {e}");
                    errors.push((repo.to_string(), e));
                }
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(())
    }
    #[allow(dead_code)]
    pub async fn add_branch_from_upstream(
        &self,
        owner: &str,
        repo: &str,
        branch_sha: &str,
    ) -> Result<(), GitError> {
        let _lock = Arc::clone(&self.semaphore).acquire_owned().await?;

        let git_ref = Reference::Branch(format!("refs/heads/{branch_sha}"));
        let octocrab = self.octocrab.clone();

        let response: Result<octocrab::models::repos::Ref, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                octocrab
                    .repos(owner, repo)
                    .create_ref(&git_ref, branch_sha)
                    .await
            },
        );
        match response {
            Ok(r) => {
                println!(
                    "Successfully created branch {} in {owner}/{repo} SHA {branch_sha}",
                    r.ref_field
                );
            }
            Err(e) => {
                eprintln!("Failed to create branch {branch_sha} in {owner}/{repo}: {e}");
                return Err(GitError::from(e));
            }
        }

        Ok(())
    }
}
