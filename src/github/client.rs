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
use crate::utils::pr::{CreatePrOptions, MergePrOptions};
use crate::utils::repo::{
    BranchProtectionRule, Checks, LicenseInfo, RepoChecks, RepoInfo, TagInfo, get_http_status,
    get_repo_info_from_url,
};
use crate::{async_retry, handle_api_response, handle_futures_unordered};
use chrono::{DateTime, Duration, Local, TimeZone, Utc};
use futures::{FutureExt, StreamExt, future::try_join, stream::FuturesUnordered};
use octocrab::Octocrab;
use octocrab::models::repos::{Ref, ReleaseNotes};
use octocrab::params::repos::Reference;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use tokio_retry::{
    RetryIf,
    strategy::{ExponentialBackoff, jitter},
};

use temp_dir::TempDir;

use indexmap::IndexSet;
use serde_json::json;

/// Contains information about tags for a forked repo, its parent,
/// and the tags that are missing from the fork
#[derive(Debug)]
pub struct Comparison {
    pub fork_tags: IndexSet<TagInfo>,
    pub parent_tags: IndexSet<TagInfo>,
    pub missing_in_fork: IndexSet<TagInfo>,
}

/// Github api entry point
pub struct GithubClient {
    /// Octocrab client. This can be trivially cloned
    pub octocrab: Octocrab,
}

impl GithubClient {
    pub fn new(github_token: String, _config: &Config, is_a_tty: bool) -> Result<Self, GitError> {
        let octocrab = Octocrab::builder()
            .personal_token(github_token.clone())
            .build()
            .map_err(GitError::GithubApiError)?;
        println!("Is user? {is_a_tty}");
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
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            println!("   Processing {repo}");
            futures.push(async move {
                let result = self.sync_fork(&repo).await;
                (repo, result)
            });
        }
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(_) => {
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
}
