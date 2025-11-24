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
use chrono::{DateTime, Duration, Utc};
use fancy_regex::Regex;
use futures::{StreamExt, stream::FuturesUnordered};
use octocrab::models::repos::ReleaseNotes;
use octocrab::params::repos::Reference;
use std::sync::OnceLock;

// Static, compiled on first use, to prevent recompiling them each time we use them
static CVE_REGEX: OnceLock<Regex> = OnceLock::new();
static ODP_REGEX: OnceLock<Regex> = OnceLock::new();

impl GithubClient {
    /// Generate release notes for a particular releaese. It grabs all the commits present in `tag`
    /// that are newer than the latest commit in `previous_tag`.
    /// This needs to be cleaned up, it is a bit of a mess right now.
    #[allow(clippy::too_many_lines)]
    pub async fn generate_release_notes(
        &self,
        url: &str,
        tag: &str,
        previous_tag: &str,
    ) -> Result<ReleaseNotes, GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);

        let octocrab = self.octocrab.clone();

        let tag_info = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let _permit = self.semaphore.clone().acquire_owned().await;
                octocrab
                    .repos(&owner, &repo)
                    .get_ref(&Reference::Tag(previous_tag.to_string()))
                    .await
            },
        )?;
        let tag_sha = match tag_info.object {
            octocrab::models::repos::Object::Commit { sha, .. }
            | octocrab::models::repos::Object::Tag { sha, .. } => sha,
            _ => String::new(),
        };
        let mut page: u32 = 1;
        let per_page = 100;
        let mut date: DateTime<Utc> = Utc::now();

        // This refers to the commits in the previous tag
        let newest_commit = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let _lock = self.semaphore.clone().acquire_owned().await;
                octocrab
                    .repos(&owner, &repo)
                    .list_commits()
                    .per_page(1)
                    .sha(tag_sha.clone())
                    .send()
                    .await
            },
        );

        if let Ok(old_commits) = newest_commit
            && let Some(commit) = old_commits.items.first()
        {
            commit.commit.committer.as_ref().map_or_else(
                || eprintln!("No commit found"),
                |c| {
                    if let Some(d) = c.date {
                        // Set the oldest commits we're going to look for to just a little bit
                        // after the most recent commit in the old tag, so that we get no overlap
                        date = d + Duration::days(1);
                    } else {
                        eprintln!("Can't get date of commit, assume things are broken");
                    }
                },
            );
        } else {
            return Err(GitError::NoSuchTag(previous_tag.to_string()));
        }

        // This is an arbitrary pre-allocation that should speed up pushing to the vec. Most of the
        // time we can assume that we'll get fewer than 300 commits difference between two
        // branches/tags. If not, pushing elements after 300 will just take slightly longer.
        let mut new_commits: Vec<String> = Vec::with_capacity(300);
        loop {
            let commits = async_retry!(
                ms = 100,
                timeout = 5000,
                retries = 3,
                error_predicate = |e: &octocrab::Error| is_retryable(e),
                body = {
                    let _lock = self.semaphore.clone().acquire_owned().await;
                    octocrab
                        .repos(&owner, &repo)
                        .list_commits()
                        .page(page)
                        .per_page(per_page)
                        .since(date)
                        .branch(tag.to_string())
                        .send()
                        .await
                },
            )?;
            // Arbitrary pre-allocated length. Uses slightly more memory, but reduces
            // the number of potential resizes
            let mut commit_line = String::with_capacity(200);
            for commit in &commits.items {
                commit_line.clear();
                if let Some(first_line) = commit.commit.message.lines().next() {
                    commit_line.push_str(first_line);
                    new_commits.push(commit_line.clone());
                }
            }

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

        let re = Regex::new(&format!("{}-[0-9]+", repo.to_ascii_uppercase()))?;
        let cve =
            CVE_REGEX.get_or_init(|| Regex::new(r"OSV|CVE").expect("Failed to compile CVE regex"));
        let odp = ODP_REGEX
            .get_or_init(|| Regex::new(r"ODP-[0-9]*").expect("Failed to compile ODP regex"));

        for s in &new_commits {
            let s_upper = s.to_ascii_uppercase();
            if cve.is_match(&s_upper)? {
                vulnerabilities.push(s.clone());
            // We don't want to match not upper case names for components, so take the line as is
            } else if re.is_match(s)? {
                component_match.push(s.clone());
            } else if odp.is_match(&s_upper)? {
                odp_changes.push(s.clone());
            } else {
                other.push(s.clone());
            }
        }

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
        ignore_previous_tag: bool,
    ) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);

        // This can fail if the previous tag doesn't exist. So instead of failing here,
        // check to see if we shuold ignore a missing previous tag and still create the new
        // release.
        let release_notes = self
            .generate_release_notes(url, current_tag, previous_tag)
            .await;

        let release_notes = if let Ok(notes) = release_notes {
            notes
        } else if ignore_previous_tag {
            eprintln!(
                "The previous tag '{previous_tag}' does not exist. Attempting to create a release for '{owner}/{repo}' without release notes."
            );
            ReleaseNotes {
                name: current_tag.to_string(),
                body: String::new(),
            }
        } else {
            return Err(GitError::NoSuchTag(previous_tag.to_string()));
        };

        let name = if let Some(release_name) = release_name {
            release_name.to_string()
        } else {
            current_tag.to_string()
        };

        // Acquire a lock on the semaphore
        let octocrab = self.octocrab.clone();
        let retries = 3;
        let release = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = retries,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let _permit = self.semaphore.clone().acquire_owned().await;
                octocrab
                    .clone()
                    .repos(&owner, &repo)
                    .releases()
                    .create(current_tag)
                    .name(&name)
                    .body(&release_notes.body)
                    .send()
                    .await
            },
        );

        match release {
            Ok(_) => {
                if self.is_tty {
                    println!("Successfully created release '{name}' for {owner}/{repo}");
                }
                self.append_slack_message(format!("Release '{name}' created for {owner}/{repo}"))
                    .await;
                Ok(())
            }
            Err(e) => {
                self.append_slack_error(format!(
                    "Failed to create release '{name}' for {owner}/{repo}"
                ))
                .await;
                eprintln!(
                    "Verify that there isn't an existing release using '{current_tag}' for {owner}/{repo}"
                );
                Err(GitError::GithubApiError(e))
            }
        }
    }
    /// Create a new release for each of the specified repositories.
    pub async fn create_all_releases(
        &self,
        current_tag: &str,
        previous_tag: &str,
        release_name: Option<&str>,
        repositories: Vec<String>,
        ignore_previous_tag: bool,
    ) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in &repositories {
            futures.push(async move {
                let result = self
                    .create_release(
                        repo,
                        current_tag,
                        previous_tag,
                        release_name,
                        ignore_previous_tag,
                    )
                    .await;
                (repo, result)
            });
        }
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            if let Err(e) = result {
                errors.push((repo.clone(), e));
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }

        Ok(())
    }
}
