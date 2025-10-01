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

use crate::async_retry;
use crate::error::{GitError, is_retryable};
use crate::github::client::GithubClient;
use crate::utils::repo::get_repo_info_from_url;
use futures::{StreamExt, stream::FuturesUnordered};
use octocrab::models::repos::Object;
use octocrab::params::repos::Reference;
use std::fmt::Display;

/// This is used to deserialize the response from the GitHub API when fetching a tag. This is
/// needed when we fetch an annotated tag.
#[derive(Debug, serde::Deserialize)]
struct GitTagObject {
    object: GitTagTarget,
}
/// This is the SHA of the actual commit the annotated tag points to.
#[derive(Debug, serde::Deserialize)]
struct GitTagTarget {
    sha: String,
}

impl GithubClient {
    /// Get the most recent commit of a branch, so we can use that to create and delete it
    pub async fn get_branch_sha<T: AsRef<str>, U: AsRef<str> + ToString>(
        &self,
        url: T,
        branch: U,
    ) -> Result<String, GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);

        // Acquire a lock on the semaphore
        let _permit = self.semaphore.clone().acquire_owned().await?;

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
                let octocrab::models::repos::Object::Commit { sha, .. } = r.object else {
                    return Err(GitError::NoSuchBranch(branch.to_string()));
                };

                Ok(sha)
            }
            Err(e) => Err(GitError::GithubApiError(e)),
        }
    }
    /// Create a branch from some base branch in a repository
    pub async fn create_branch(
        &self,
        url: impl AsRef<str>,
        base_branch: impl AsRef<str> + ToString + Display,
        new_branch: impl AsRef<str> + ToString + Display,
        quiet: bool,
    ) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);

        // Acquire a lock on the semaphore
        let _permit = self.semaphore.clone().acquire_owned().await?;

        let res: Result<_, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                self.octocrab
                    .clone()
                    .repos(&owner, &repo)
                    .get_ref(&Reference::Branch(base_branch.to_string()))
                    .await
            },
        );
        let response = match res {
            Ok(r) => r,
            Err(e) => {
                self.append_slack_error(format!(
                    "{owner}/{repo}: Unable to fetch tag {base_branch} from Github: {e}"
                ))
                .await;

                return Err(GitError::GithubApiError(e));
            }
        };

        let octocrab::models::repos::Object::Commit { sha, .. } = response.object else {
            self.append_slack_error(format!("{owner}/{repo}: No such branch {base_branch}"))
                .await;

            return Err(GitError::NoSuchBranch(base_branch.to_string()));
        };

        let res: Result<_, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                self.octocrab
                    .clone()
                    .repos(&owner, &repo)
                    .create_ref(&Reference::Branch(new_branch.to_string()), sha.clone())
                    .await
            },
        );
        match res {
            Ok(_) => {
                if !quiet {
                    self.append_slack_message(format!("New branch {new_branch} created"))
                        .await;
                }
                Ok(())
            }
            Err(e) => {
                self.append_slack_error(format!(
                    "{owner}/{repo}: Failed to create {new_branch}: {e}"
                ))
                .await;

                Err(GitError::GithubApiError(e))
            }
        }
    }
    /// Create a branch from some base branch in a repository
    pub async fn create_branch_from_tag(
        &self,
        url: impl AsRef<str>,
        base_tag: impl AsRef<str> + ToString + Display,
        new_branch: impl AsRef<str> + ToString + Display,
        quiet: bool,
    ) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);

        // Acquire a lock on the semaphore
        let _permit = self.semaphore.clone().acquire_owned().await?;
        let octocrab = self.octocrab.clone();

        // We have to match against the SHA of the commit the tag points to, which is why we have
        // to check if we're working with a lightweight or annotated tag.
        let res: Result<Option<String>, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let tag_ref = octocrab
                    .repos(&owner, &repo)
                    .get_ref(&Reference::Tag(base_tag.to_string()))
                    .await;
                match tag_ref {
                    Ok(t) => match t.object {
                        Object::Commit { sha, .. } => Ok(Some(sha)),
                        Object::Tag { sha, .. } => {
                            let tag_opt: Result<GitTagObject, octocrab::Error> = octocrab
                                .get(format!("/repos/{owner}/{repo}/git/tags/{sha}"), None::<&()>)
                                .await;
                            if let Ok(tag) = tag_opt {
                                Ok(Some(tag.object.sha))
                            } else {
                                Ok(None)
                            }
                        }
                        _ => Ok(None),
                    },
                    Err(e) => {
                        return Err(e);
                    }
                }
            },
        );
        let response = match res {
            Ok(Some(r)) => r,
            Ok(None) => {
                eprintln!("Error: Could not get SHA for {base_tag}");
                self.append_slack_error(format!(
                    "{owner}/{repo}: Unable to get SHA for tag '{base_tag}'"
                ))
                .await;
                return Err(GitError::Other("Unable to get tag".to_string()));
            }
            Err(e) => {
                eprintln!("ERROR: {e}");
                self.append_slack_error(format!(
                    "{owner}/{repo}: Unable to fetch tag {base_tag} from Github: {e}"
                ))
                .await;
                return Err(GitError::GithubApiError(e));
            }
        };

        if response.is_empty() {
            eprintln!("Unable to get SHA for tag {base_tag}");
            self.append_slack_error(format!(
                "{owner}/{repo}: Unable to get SHA for '{base_tag}'"
            ))
            .await;
            return Err(GitError::NoSuchTag(base_tag.to_string()));
        }

        let res: Result<_, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                self.octocrab
                    .clone()
                    .repos(&owner, &repo)
                    .create_ref(&Reference::Branch(new_branch.to_string()), response.clone())
                    .await
            },
        );
        match res {
            Ok(_) => {
                if !quiet {
                    self.append_slack_message(format!(
                        "{owner}/{repo} - New branch {new_branch} created"
                    ))
                    .await;
                }
                Ok(())
            }
            Err(e) => {
                self.append_slack_error(format!(
                    "{owner}/{repo}: Failed to create {new_branch}: {e}"
                ))
                .await;
                match e {
                    octocrab::Error::GitHub { source, .. } => {
                        eprintln!("{source:#?}");
                        return Ok(());
                    }
                    _ => eprintln!("{e}"),
                }
                Err(GitError::GithubApiError(e))
            }
        }
    }
    /// Create the passed branch for each repository provided
    pub async fn create_all_branches<T: AsRef<str> + Display + Copy, U: AsRef<str> + Display>(
        &self,
        base_branch: T,
        base_tag: T,
        new_branch: T,
        repositories: &[U],
        quiet: bool,
    ) -> Result<(), GitError> {
        println!("Base tag is {base_tag}");
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            let base_branch = base_branch.to_string();
            let base_tag = base_tag.to_string();
            futures.push(async move {
                let result = if base_tag.is_empty() {
                    println!("Creating {new_branch} from a {base_branch}");
                    self.create_branch(repo, &base_branch, &new_branch, quiet)
                        .await
                } else {
                    println!("Creating {new_branch} from a {base_tag}");
                    self.create_branch_from_tag(repo, &base_tag, &new_branch, quiet)
                        .await
                };
                (repo, result)
            });
        }

        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(()) => {
                    println!("✅ Successfully created '{new_branch}' for {repo}");
                }
                Err(e) => {
                    eprintln!("❌ Failed to create '{new_branch}' for {repo}");
                    errors.push((repo.to_string(), e));
                }
            }
        }

        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(())
    }
    /// Delete a branch from a repository
    pub async fn delete_branch<
        T: AsRef<str> + ToString + Display,
        U: AsRef<str> + ToString + Display,
    >(
        &self,
        url: T,
        branch: U,
    ) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        // Acquire a lock on the semaphore
        let _permit = self.semaphore.clone().acquire_owned().await?;
        let result: Result<_, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                self.octocrab
                    .clone()
                    .repos(&owner, &repo)
                    .delete_ref(&Reference::Branch(branch.to_string()))
                    .await
            },
        );

        match result {
            Ok(()) => {
                println!("✅ Successfully deleted branch '{branch}' for {repo}");
                Ok(())
            }
            Err(e) => {
                if let octocrab::Error::GitHub { source, .. } = &e
                    && source.message.contains("Reference does not exist")
                {
                    // Branch doesn't exist, so we can consider this a success
                    eprintln!("Branch '{branch}' already exists, ignoring");
                    return Ok(());
                }
                eprintln!("❌ Failed to delete branch '{branch}' for {repo}: {e}");
                self.append_slack_error(format!(
                    "{owner}/{repo}: Failed to delete branch {branch}: {e}"
                ))
                .await;
                Err(GitError::GithubApiError(e))
            }
        }
    }
    /// Delete the specified branch for each configured repository
    pub async fn delete_all_branches<
        T: AsRef<str> + ToString + Display + Copy,
        U: AsRef<str> + ToString + Display,
    >(
        &self,
        branch: T,
        repositories: &[U],
    ) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            futures.push(async move {
                let result = self.delete_branch(repo, branch).await;
                // Drop this variable after the above function has finished.
                // Free a slot for another job to run
                (repo, result)
            });
        }

        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(()) => {
                    println!("✅ Successfully deleted '{branch}' for {repo}");
                }
                Err(e) => {
                    println!("❌ Failed to delete '{branch}' for {repo}");
                    errors.push((format!("{repo} ({branch})"), e));
                }
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(())
    }
}
