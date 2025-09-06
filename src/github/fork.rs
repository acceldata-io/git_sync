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
use futures::{StreamExt, stream::FuturesUnordered};

impl GithubClient {
    /// Sync a single repository with its parent repository.
    pub async fn sync_fork(&self, url: &str) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        println!("Syncing {owner}/{repo} with its parent repository...");

        let parent = self.get_parent_repo(url).await?;
        let body = serde_json::json!({"branch": parent.main_branch});
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

    /// Sync all configured repositories. Only repositories that have a parent repository
    /// should be passed to this function
    pub async fn sync_all_forks(&self, repositories: Vec<String>) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            let semaphore_permit = self.semaphore.clone().acquire_owned().await?;
            futures.push(async move {
                let result = self.sync_fork(&repo).await;
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
}
