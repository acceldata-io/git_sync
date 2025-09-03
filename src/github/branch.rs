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
use crate::{async_retry, handle_api_response, handle_futures_unordered};
use futures::{FutureExt, StreamExt, future::try_join, stream::FuturesUnordered};
use octocrab::params::repos::Reference;

impl GithubClient {
    /// Get the most recent commit of a branch, so we can use that to create and delete it
    pub async fn get_branch_sha(&self, url: &str, branch: &str) -> Result<String, GitError> {
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
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            println!("   Processing {repo}");

            futures.push(async move {
                let result = self.create_branch(repo, base_branch, new_branch).await;
                (repo, result)
            });
        }

        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(_) => {
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
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            println!("   Processing {repo}");
            println!("   Processing {repo}");
            futures.push(async move {
                let result = self.delete_branch(repo, branch).await;
                (repo, result)
            });
        }

        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(_) => {
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
