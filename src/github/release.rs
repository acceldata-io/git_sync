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
use chrono::{DateTime, Duration, Utc};
use futures::{FutureExt, StreamExt, future::try_join, stream::FuturesUnordered};
use octocrab::models::repos::ReleaseNotes;
use octocrab::params::repos::Reference;
use regex::Regex;
use std::sync::OnceLock;
use tokio_retry::{
    RetryIf,
    strategy::{ExponentialBackoff, jitter},
};

// Static, compiled on first use, to prevent recompiling them each time we use them
static CVE_REGEX: OnceLock<Regex> = OnceLock::new();
static ODP_REGEX: OnceLock<Regex> = OnceLock::new();

impl GithubClient {
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
}
