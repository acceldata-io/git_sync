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
use serde_json::json;
use std::collections::HashMap;

/// Whether GitHub refused a merge because of a branch protection rule or a ruleset, rather than
/// because of the state of the branches themselves. An unmet review or status check requirement
/// comes back as a 405, and a branch that cannot be written to comes back as a 403.
fn is_blocked_by_branch_rules(e: &octocrab::Error) -> bool {
    let octocrab::Error::GitHub { source, .. } = e else {
        return false;
    };
    if source.status_code == http::StatusCode::METHOD_NOT_ALLOWED {
        // A conflicted pull request is reported as a 405 too, and writing to the base branch
        // cannot resolve that.
        return !source.message.to_lowercase().contains("not mergeable");
    }
    source.status_code == http::StatusCode::FORBIDDEN
}

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
                        head: opts.head.clone(),
                        base: opts.base.clone(),
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
                url: repo.clone(),
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
                        pr_map.insert(repo.clone(), pr_number);
                    }
                    Err(e) => errors.push((repo.clone(), e)),
                }
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }

        Ok(pr_map)
    }

    /// Merge a pull request. This will only work if there are no merge conflicts in the pull
    /// request. If `opts.force` is set and GitHub refuses the merge because of a branch protection
    /// rule or ruleset, this falls back to [`Self::force_merge_pr`].
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
                if opts.force && is_blocked_by_branch_rules(&e) {
                    eprintln!(
                        "Branch rules refused the merge of PR #{pr_number} in {owner}/{repo} ({e}); merging into the base branch directly instead"
                    );
                    return self.force_merge_pr(opts, &owner, &repo).await;
                }
                self.append_slack_error(format!(
                    "Failed to merge PR #{pr_number} in {owner}/{repo}: {e}"
                ))
                .await;
                Err(GitError::PRNotMergeable(pr_number))
            }
        }
    }

    /// Merge the head branch of a pull request straight into its base branch, using GitHub's
    /// "merge a branch" endpoint instead of the pull request merge endpoint.
    ///
    /// GitHub evaluates this as a write to the base branch rather than as a pull request merge, so
    /// it goes through for accounts that are allowed to bypass the pull request requirements on
    /// that branch even when the required approving review is missing. Once the head commit is
    /// reachable from the base branch, GitHub closes the pull request as merged by itself.
    ///
    /// This always produces a merge commit, so the requested merge method is not honoured here.
    async fn force_merge_pr(
        &self,
        opts: &MergePrOptions,
        owner: &str,
        repo: &str,
    ) -> Result<(), GitError> {
        let pr_number = opts.pr_number;
        let octocrab = self.octocrab.clone();

        let pull_request: Result<_, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let _permit = self.semaphore.clone().acquire_owned().await;
                octocrab.pulls(owner, repo).get(pr_number).await
            },
        );
        let pull_request = pull_request.map_err(|_| GitError::PRNotMergeable(pr_number))?;

        let base = pull_request.base.ref_field.clone();
        let head_branch = pull_request.head.ref_field.clone();
        // Prefer the SHA the merge was validated against, so that a commit pushed while this is
        // running cannot slip into the base branch.
        let head = opts
            .sha
            .clone()
            .unwrap_or_else(|| pull_request.head.sha.clone());

        let title = opts.title.as_deref().map(str::trim).filter(|t| !t.is_empty());
        let detail = opts
            .message
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty());
        let commit_message = match (title, detail) {
            (Some(title), Some(detail)) => format!("{title}\n\n{detail}"),
            (Some(title), None) => title.to_string(),
            (None, Some(detail)) => {
                format!("Merge pull request #{pr_number} from {head_branch}\n\n{detail}")
            }
            (None, None) => format!("Merge pull request #{pr_number} from {head_branch}"),
        };

        let body = json!({
            "base": &base,
            "head": &head,
            "commit_message": commit_message,
        });

        let response: Result<serde_json::Value, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let _permit = self.semaphore.clone().acquire_owned().await;
                octocrab
                    .post::<serde_json::Value, _>(
                        format!("/repos/{owner}/{repo}/merges"),
                        Some(&body),
                    )
                    .await
            },
        );

        match response {
            Ok(_) => {
                println!(
                    "Merged '{head_branch}' into '{base}' directly for {owner}/{repo}; PR #{pr_number} will be closed as merged"
                );
                self.append_slack_message(format!(
                    "PR #{pr_number} in {owner}/{repo} was merged by writing to '{base}' directly, bypassing its branch rules"
                ))
                .await;
                Ok(())
            }
            Err(e) => {
                self.append_slack_error(format!(
                    "Failed to merge PR #{pr_number} in {owner}/{repo} directly into '{base}': {e}"
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
