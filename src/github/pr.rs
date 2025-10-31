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
use crate::utils::pr::{CreatePrOptions, MergePrOptions};
use crate::utils::repo::get_repo_info_from_url;
use futures::{StreamExt, stream::FuturesUnordered};
use std::collections::HashMap;

impl GithubClient {
    /// Create a pull request for a specific repository
    #[allow(clippy::too_many_lines)]
    pub async fn create_pr(
        &self,
        opts: &CreatePrOptions,
    ) -> Result<Option<(u64, String)>, GitError> {
        let info = get_repo_info_from_url(&opts.url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let octocrab = self.octocrab.clone();

        let retries = 3;

        // Verify that head and base have difference. If they don't, skip creating a PR since it's
        // not necessary
        let difference: Result<_, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = retries,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let _permit = self.semaphore.clone().acquire_owned().await;
                octocrab
                    .commits(&owner, &repo)
                    .compare(&opts.base, &opts.head)
                    .send()
                    .await
            },
        );
        match difference {
            Ok(compare) => {
                if compare.ahead_by == 0 {
                    eprintln!(
                        "No differences between {} and {} in {}/{} - skipping PR creation",
                        opts.head, opts.base, owner, repo
                    );
                    return Ok(None);
                }
            }
            Err(e) => {
                eprintln!("Failed to compare branches for {owner}/{repo}: {e}");
            }
        }

        let mut pr_number: Option<u64> = None;
        let pr_result: Result<_, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = retries,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let _permit = self.semaphore.clone().acquire_owned().await;
                octocrab
                    .pulls(&owner, &repo)
                    .create(&opts.title, &opts.head, &opts.base)
                    .body(opts.body.as_deref().unwrap_or_default())
                    .send()
                    .await
            },
        );
        match pr_result {
            Ok(p) => {
                pr_number = Some(p.number);
                println!("PR #{} created successfully for {owner}/{repo}", p.number);
            }
            Err(e) => {
                if let octocrab::Error::GitHub { source, .. } = &e
                    && source.status_code == 422
                {
                    eprintln!(
                        "PR may already exist for {owner}/{repo}:{} - attempting to proceed",
                        opts.head
                    );
                } else {
                    eprintln!("Failed to create PR for {owner}/{repo}: {e}");
                    self.append_slack_error(format!("Failed to create PR for {owner}/{repo}: {e}"))
                        .await;
                    return Err(GitError::GithubApiError(e));
                }
            }
        }

        let pr_number = if let Some(number) = pr_number {
            number
        } else {
            let pr_result: Result<_, octocrab::Error> = async_retry!(
                ms = 100,
                timeout = 5000,
                retries = retries,
                error_predicate = |e: &octocrab::Error| is_retryable(e),
                body = {
                    let _permit = self.semaphore.clone().acquire_owned().await;
                    octocrab
                        .pulls(&owner, &repo)
                        .list()
                        .head(format!("{owner}:{}", opts.head))
                        .base(&opts.base)
                        .per_page(1)
                        .send()
                        .await
                },
            );
            match pr_result {
                Ok(p) => p
                    .items
                    .first()
                    .map(|pr| pr.number)
                    .ok_or_else(|| GitError::NoSuchPR {
                        repository: format!("{owner}/{repo}"),
                        head: opts.head.to_string(),
                        base: opts.base.to_string(),
                    })?,
                Err(e) => {
                    self.append_slack_error(format!(
                        "Failed to get existing PR number for {owner}/{repo}: {e}"
                    ))
                    .await;
                    return Err(GitError::GithubApiError(e));
                }
            }
        };
        if !opts.should_merge {
            return Ok(None);
        }
        let branch_sha = self.get_branch_sha(&opts.url, &opts.head).await?;
        let commit_sha: Result<_, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = retries,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let _permit = self.semaphore.clone().acquire_owned().await;
                octocrab
                    .repos(&owner, &repo)
                    .list_commits()
                    .branch(&branch_sha)
                    .per_page(1)
                    .send()
                    .await
            },
        );
        let sha = match commit_sha {
            Ok(p) => {
                if let Some(commit) = p.items.first() {
                    commit.sha.clone()
                } else {
                    return Err(GitError::Other(format!(
                        "Cannot get sha of latest commit for {owner}/{repo}"
                    )));
                }
            }
            Err(e) => return Err(GitError::GithubApiError(e)),
        };

        if let Some(reviewers) = opts.reviewers.as_deref() {
            let reviewer_result = async_retry!(
                ms = 100,
                timeout = 5000,
                retries = 3,
                error_predicate = |e: &octocrab::Error| is_retryable(e),
                body = {
                    let _permit = self.semaphore.clone().acquire_owned().await;
                    octocrab
                        .clone()
                        .pulls(&owner, &repo)
                        .request_reviews(pr_number, reviewers, &[])
                        .await
                },
            );

            match reviewer_result {
                Ok(_) => println!(
                    "Successfully requested reviewers for PR #{pr_number} in {owner}/{repo}"
                ),
                Err(e) => eprintln!(
                    "Failed to request reviewers for PR #{pr_number} in {owner}/{repo}: {e}"
                ),
            }
        }
        Ok(Some((pr_number, sha)))
    }
    /// Create a pull request for all configured repositories, and optionally merge them
    /// automatically, if possible
    pub async fn create_all_prs(
        &self,
        opts: &CreatePrOptions,
        merge_opts: Option<MergePrOptions>,
        repositories: Vec<String>,
    ) -> Result<HashMap<String, u64>, GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in &repositories {
            let merge_opts = merge_opts.clone();
            // Copy the fields of the opts struct, except for what we need to override (namely, the
            // url)
            let pr_opts = CreatePrOptions {
                url: repo.to_string(),
                ..opts.clone()
            };
            futures.push(async move {
                let result = self.create_pr(&pr_opts).await;

                match result {
                    Ok(Some((pr_number, sha))) => {
                        if let Some(mut opts) = merge_opts {
                            opts.url.clone_from(repo);
                            opts.pr_number = pr_number;
                            opts.sha = Some(sha);
                            let merge_result = self.merge_pr(&opts).await;
                            Some((repo, merge_result.map(|()| pr_number)))
                        } else {
                            Some((repo, Ok(pr_number)))
                        }
                    }
                    Ok(None) => None,
                    Err(e) => Some((repo, Err(e))),
                }
            });
        }

        // Keep track of which PR number belongs to which repository
        let mut pr_map: HashMap<String, u64> = HashMap::new();
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some(res) = futures.next().await {
            if let Some((repo, result)) = res {
                match result {
                    Ok(pr_number) => {
                        pr_map.insert(repo.to_string(), pr_number);
                    }
                    Err(e) => errors.push((repo.to_string(), e)),
                }
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }

        Ok(pr_map)
    }

    /// Merge a pull request. This will only work if there are no merge conflicts in the pull
    /// request.
    pub async fn merge_pr(&self, opts: &MergePrOptions) -> Result<(), GitError> {
        let info = get_repo_info_from_url(&opts.url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let pr_number = opts.pr_number;
        let octocrab = self.octocrab.clone();
        let merge_result: Result<_, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let _permit = self.semaphore.clone().acquire_owned().await;
                octocrab
                    .pulls(&owner, &repo)
                    .merge(opts.pr_number)
                    .message(opts.message.as_deref().unwrap_or_default())
                    .title(opts.title.as_deref().unwrap_or_default())
                    .sha(opts.sha.as_deref().unwrap_or_default())
                    .method(opts.method)
                    .send()
                    .await
            },
        );

        match merge_result {
            Ok(_) => {
                println!("Successfully merged PR #{pr_number} in {repo}");
                Ok(())
            }
            Err(e) => {
                self.append_slack_error(format!(
                    "Failed to merge PR #{pr_number} in {owner}/{repo}: {e}"
                ))
                .await;
                Err(GitError::PRNotMergeable(pr_number))
            }
        }
    }

    /*
    // Merge all PRs in the provided repositories
    pub async fn merge_all_prs(
        &self,
        opts: MergePrOptions,
        repositories: HashMap<String, u64>,
    ) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for (repo, pr_number) in repositories.into_iter() {
            let merge_opts = MergePrOptions {
                url: repo.clone(),
                pr_number,
                ..opts.clone()
            };

            futures.push(async move {
                let result = self.merge_pr(&merge_opts).await;
                (repo, result)
            });
        }

        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(_) => println!("Successfully merged PR #{} in {repo}", opts.pr_number),
                Err(e) => eprintln!("Failed to merge PR #{} in {repo}: {e}", opts.pr_number),
            }
        }
        Ok(())
    }
    */
}
