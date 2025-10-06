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
use crate::config::Config;
use crate::error::{GitError, is_retryable};
use crate::utils::repo::{RepoInfo, TagInfo, get_repo_info_from_url};
use chrono::{DateTime, Local, TimeZone, Utc};
use octocrab::Octocrab;
use tokio::sync::Mutex;

use indexmap::IndexSet;
use tokio::sync::Semaphore;

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::IsTerminal;
use std::sync::Arc;
use tokio::sync::OnceCell;

/// Contains information about tags that are missing for a forked repo.
#[derive(Debug)]
pub struct Comparison {
    /// An ordered set of tags that are missing in the fork repository
    pub missing_in_fork: IndexSet<TagInfo>,
    /// A hashset of the parent repositories. If you have a fork of a fork, you might have more
    /// than one parent URL.
    pub parent_urls: HashSet<String>,
}

/// Github api entry point
pub struct GithubClient {
    /// Octocrab client. This can be trivially cloned
    pub octocrab: Octocrab,
    /// A semaphore to control the maximum number of jobs that can be run in parallel
    pub semaphore: Arc<Semaphore>,
    /// An optional webook for posting to slack, if that feature is enabled
    pub webhook_url: String,
    /// Defines whether or not we're running in interactive mode
    pub is_tty: bool,
    /// Where output should go. This can only be written to Once
    pub output: Arc<OnceCell<OutputMode>>,
    /// A message that gets sent to slack at the end of all processing, if it has any contents.
    /// This is thread safe.
    slack_messages: Arc<Mutex<Vec<String>>>,
    /// An error message that will get sent to slack at the end of all processing,
    /// if it has any contents. This field is also thread safe.
    slack_errors: Arc<Mutex<Vec<String>>>,
}
/// This is sort of implemented. Some places are using this, some are ignoring it completely. This
/// is ripe for future polish.
#[derive(Default, PartialEq, Eq)]
pub enum OutputMode {
    #[default]
    /// No output
    None,
    /// Print output to stdout. This is useful for when we do want info in the console, but aren't
    /// a tty, such as when we're piping output to some other command
    Print,
    /// This is to display a progress bar. Some actions take a very long time and this is a way to
    /// make it clear the program hasn't hanged. This should only be enabled when this is being run
    /// in an interactive terminal.
    Progress,
}

impl GithubClient {
    /// Initialize a new Github client
    pub fn new<T: AsRef<str>>(
        github_token: T,
        // Not currently used
        _config: &Config,
        max_jobs: usize,
        slack_webhook: Option<String>,
    ) -> Result<Self, GitError> {
        let octocrab = Octocrab::builder()
            .personal_token(github_token.as_ref())
            .build()
            .map_err(GitError::GithubApiError)?;
        let webhook_url: String = slack_webhook.unwrap_or_default().trim().to_string();

        Ok(Self {
            octocrab,
            semaphore: Arc::new(Semaphore::new(max_jobs)),
            webhook_url,
            is_tty: std::io::stdout().is_terminal(),
            // This is a type that can only be written to once, and represents the type of output
            // that we have.
            output: Arc::new(OnceCell::new()),
            // Arbitrary initial capacity to avoid allocations if there aren't that many messages
            slack_messages: Arc::new(Mutex::new(Vec::with_capacity(100))),
            slack_errors: Arc::new(Mutex::new(Vec::with_capacity(100))),
        })
    }
    /// Get the parent repository of a github repository.
    pub async fn get_parent_repo<T: AsRef<str>>(&self, url: T) -> Result<RepoInfo, GitError> {
        let info = get_repo_info_from_url(url.as_ref())?;
        let (owner, repo) = (info.owner, info.repo_name);
        let octocrab = self.octocrab.clone();

        let repo_info = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let _permit = self.semaphore.clone().acquire_owned().await;
                let owner = owner.clone();
                let repo = repo.clone();
                octocrab.repos(owner, repo).get().await
            },
        )?;

        let parent = *repo_info.parent.ok_or(GitError::NotAFork)?;

        let parent_owner = parent.owner.ok_or(GitError::NoUpstreamRepo)?.login;

        let url = parent
            .html_url
            .ok_or_else(|| GitError::InvalidRepository(url.as_ref().to_string()))?
            .to_string();
        Ok(RepoInfo {
            owner: parent_owner,
            repo_name: parent.name,
            url,
            main_branch: parent.default_branch,
        })
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
                eprintln!(
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
        let query = r"
            {
                rateLimit {
                    limit
                    remaining
                    resetAt
                }
            }";
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

                eprintln!(
                    "GraphQL API Rate limit: {remaining}/{limit} remaining. Resets at {local_time} ({time_zone})"
                );
                Ok(())
            }
            Err(e) => Err(GitError::GithubApiError(e)),
        }
    }
    /// Appends success messages to be sent to slack at the end. The message will only be sent if
    /// slack integration is enabled and the webhook url is set.
    pub async fn append_slack_message<T: Into<String>>(&self, msg: T) {
        #[cfg(feature = "slack")]
        {
            let mut message = self.slack_messages.lock().await;
            message.push(msg.into());
        }
    }
    /// Append an error message to be sent to slack at the end. The message will only be sent if
    /// slack integartion is enabled and the webhhook url is set.
    pub async fn append_slack_error<T: Into<String>>(&self, msg: T) {
        #[cfg(feature = "slack")]
        {
            let mut message = self.slack_errors.lock().await;
            message.push(msg.into());
        }
    }
    /// This can be used to send the contents of `slack_messages` and `slack_errors` to slack in
    /// one batch.
    pub async fn slack_message(&self) {
        // If slack support is enabled, post the message
        #[cfg(feature = "slack")]
        {
            use crate::slack::post_message::send_slack_message;
            use std::collections::HashMap;

            let mut slack_message = self.slack_messages.lock().await;
            let mut slack_errors = self.slack_errors.lock().await;
            let message_len = slack_message.len();
            let error_len = slack_errors.len();

            // There's no point in continuing if there are no messages or errors
            if message_len == 0 && error_len == 0 {
                return;
            }

            let now = Utc::now();

            let date = now.format("%H:%M (UTC) %d-%m-%Y").to_string();

            // Arbitrary initial capacity
            let mut message = String::with_capacity(256);
            let _ = write!(message, "---\nRun completed at {date}\n\n");

            if message_len > 0 {
                slack_message.sort();
                for msg in slack_message.iter() {
                    message.push_str("* ");
                    message.push_str(msg);
                    message.push('\n');
                }
            }
            if error_len > 0 {
                slack_errors.sort();
                if message_len > 0 {
                    message.push('\n');
                }
                message.push_str("*Errors*:\n");
                for err in slack_errors.iter() {
                    let first_line = err.lines().next().unwrap_or(err);
                    message.push_str("* ");
                    message.push_str(first_line);
                    message.push('\n');
                }
            }

            message.push_str("---");
            if !message.is_empty() && !self.webhook_url.is_empty() {
                // Post the message to slack
                let mut map = HashMap::new();
                map.insert("text".to_string(), message);
                let response = send_slack_message(self.webhook_url.clone(), map).await;
                if response.is_err() {
                    eprintln!(
                        "Failed to post message to slack: {}",
                        response.err().unwrap()
                    );
                }
            }
        }
    }
}
