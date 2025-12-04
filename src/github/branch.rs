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
use crate::utils::file_utils::{replace_all_in_directory, replace_all_in_directory_with};
use crate::utils::filter::{filter_ref, get_or_compile};
use crate::utils::repo::{get_repo_info_from_url, http_to_ssh_repo};
use fancy_regex::Captures;
use futures::{StreamExt, stream::FuturesUnordered};
use octocrab::models::repos::Object;
use octocrab::params::repos::Reference;
use std::fmt::Display;
use std::fs;
use std::process::Command;
use tempfile::TempDir;
use walkdir::WalkDir;

use std::collections::HashMap;
use std::hash::Hash;
use std::path::PathBuf;

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
/// An enum to get typed results from modifying a branch instead of using strings.
#[derive(Debug)]
enum ModificationResult {
    DryRun(PathBuf),
    NoChanges,
    Success,
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

        let res: Result<_, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let _lock = self.semaphore.clone().acquire_owned().await;
                self.octocrab
                    .clone()
                    .repos(&owner, &repo)
                    .get_ref(&Reference::Branch(branch.to_string()))
                    .await
            },
        );

        match res {
            Ok(r) => {
                let Object::Commit { sha, .. } = r.object else {
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

        let res: Result<_, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let _lock = self.semaphore.clone().acquire_owned().await;
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

        let Object::Commit { sha, .. } = response.object else {
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
                let _lock = self.semaphore.clone().acquire_owned().await;
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
                    self.append_slack_message(format!(
                        "{owner}/{repo}: New branch '{new_branch}' created"
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
        let octocrab = self.octocrab.clone();

        // We have to match against the SHA of the commit the tag points to, which is why we have
        // to check if we're working with a lightweight or annotated tag.
        let res: Result<Option<String>, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let permit = self.semaphore.clone().acquire_owned().await;
                let tag_ref = octocrab
                    .repos(&owner, &repo)
                    .get_ref(&Reference::Tag(base_tag.to_string()))
                    .await;
                drop(permit);
                match tag_ref {
                    Ok(t) => match t.object {
                        Object::Commit { sha, .. } => Ok(Some(sha)),
                        // TODO: See if the macro can be nested, so the below can be retried as
                        // well
                        Object::Tag { sha, .. } => {
                            let _permit = self.semaphore.clone().acquire_owned().await;
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
                return Err(GitError::NoSuchTag(base_tag.to_string()));
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
                let _lock = self.semaphore.clone().acquire_owned().await;
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
    pub async fn delete_branch<T, U>(&self, url: T, branch: U) -> Result<(), GitError>
    where
        T: AsRef<str> + ToString + Display,
        U: AsRef<str> + ToString + Display,
    {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        // Acquire a lock on the semaphore
        let result: Result<_, octocrab::Error> = async_retry!(
            ms = 100,
            timeout = 5000,
            retries = 3,
            error_predicate = |e: &octocrab::Error| is_retryable(e),
            body = {
                let _permit = self.semaphore.clone().acquire_owned().await;
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
                    eprintln!("Branch '{branch}' does not exist, ignoring");
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
    //async fn replace_in_file()
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

    /// Clones the specified branch from a repository, updates the release version in all files,
    /// renames package files if necessary, commits, and pushes the changes.
    /// Returns an error if any step fails.
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    pub async fn modify_branch(
        &self,
        url: String,
        branch: String,
        old_text: String,
        new_text: String,
        message: Option<String>,
        dry_run: bool,
        is_version: bool,
        quiet: bool,
    ) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (repo, owner, url) = (info.repo_name, info.owner, info.url);
        let repo_slack = repo.clone();
        let old_text_slack = old_text.clone();
        let new_text_slack = new_text.clone();
        let owner_slack = owner.clone();
        let branch_slack = branch.clone();

        // Acquire a lock on the semaphore to limit concurrent operations
        let permit = self.semaphore.clone().acquire_owned().await?;

        let url = http_to_ssh_repo(&url)?;

        let result = tokio::task::spawn_blocking(move || {
            let _lock = permit;
            let tmp_dir = TempDir::new()?;

            let tmp = if dry_run {
                tmp_dir.keep()
            } else {
                tmp_dir.path().into()
            };

            let repo_dir = tmp.join(&repo);
            println!(
                "Starting to clone {repo}'s branch {branch} into {}",
                tmp.display()
            );

            // Clone the specified branch into a temporary directory
            // We use --depth 1 to only get the latest commit
            // We use --single-branch to only get the specified branch
            // We use --branch to specify the branch to clone
            let git_status = Command::new("git")
                .arg("clone")
                .arg("--branch")
                .arg(&branch)
                .arg("--single-branch")
                .arg("--depth")
                .arg("1")
                .arg(&url) // takes &String fine
                .current_dir(&tmp)
                .status()?;

            if git_status.success() {
                println!("Cloned {repo}'s branch {branch} into {}", tmp.display());
            } else {
                return Err(GitError::GitCloneError {
                    repository: repo,
                    branch,
                });
            }

            // We can't recover from a failed regex compilation, so panic lazily (ie don't allocate
            // unless there's an error).
            let re = get_or_compile(fancy_regex::escape(&old_text))
                .unwrap_or_else(|_| panic!("Unable to compile regex using '{old_text}'"));
            replace_all_in_directory(&repo_dir, &re, new_text.as_str(), quiet);

            // If we're working with the odp-bigtop repo, we also need to rename package files
            // in bigtop-packages/src/deb
            if repo == "odp-bigtop" && is_version {
                let is_odp = get_or_compile(r"(?P<odp>[0-9][.][0-9][.][0-9][.][0-9])-([0-9]+)")
                    .unwrap_or_else(|_| panic!("Unable to compile ODP regex"));
                if let Some(captures) = is_odp.captures(&fancy_regex::escape(&old_text))? {
                    let odp_version = match captures.name("odp") {
                        Some(m) => m.as_str().to_string(),
                        None => return Err(GitError::Other("Some error occurred while determining the ODP VERSION".to_string())),
                    };

                    let odp_regex_text = format!(r#"^\s*(version|VERSION|ODP_VERSION|odp_version)\s*=\s*\"?{odp_version}?""#);
                    let odp_version_re = get_or_compile(odp_regex_text)?;
                    let new_odp_version = new_text.split('-').next().ok_or_else(|| GitError::Other("Some error occurred while determining the ODP version".to_string()))?;

                    let replacement = |captures: &Captures| format!(r#"{} = "{}"#, &captures[1], new_odp_version);
                    replace_all_in_directory_with(
                        &repo_dir,
                        &odp_version_re,
                        &replacement,
                        quiet,
                    );
                }
                let build_number = new_text
                    .split('-')
                    .next_back()
                    .ok_or_else(|| GitError::InvalidBigtopVersion(new_text.clone()))?;

                let bn_regex_string = r#"(odp_bn|ODP_BN)\s*=\s*"\d+";"#;
                let bn_regex = get_or_compile(bn_regex_string).unwrap_or_else(|_| {
                    panic!("Unable to compile ODP build number regex '{bn_regex_string}'")
                });

                // This grabs the capture group, which will be either ODP_BN or odp_bn, then uses
                // a closure to correctly format the replacement string.
                replace_all_in_directory_with(
                    &repo_dir,
                    &bn_regex,
                    &|caps: &Captures| format!(r#"{} = "{}";"#, &caps[1], build_number),
                    quiet,
                );

                let dir = format!("{}/bigtop-packages/src/deb", repo_dir.display());

                let old_text_dash = old_text.replace('.', "-");
                let new_text_dash = new_text.replace('.', "-");

                let odp_version_dashes_re = get_or_compile(fancy_regex::escape(&old_text_dash))
                    .unwrap_or_else(|_| {
                        panic!(
                            "Unable to compile ODP version with dashes regex using '{old_text_dash}'"
                        )
                    });

                replace_all_in_directory(
                    &repo_dir,
                    &odp_version_dashes_re,
                    &new_text_dash,
                    quiet,
                );

                let bigtop_re_text =
                    format!("^(.*)-{}([.-].*)(.*)", fancy_regex::escape(&old_text_dash));

                let bigtop_re = get_or_compile(&bigtop_re_text).unwrap_or_else(|_| {
                    panic!("Failed to compile old version regex '{bigtop_re_text}'")
                });


                for entry in WalkDir::new(&dir).into_iter().filter_map(Result::ok) {
                    let path = entry.path();
                    if path.is_dir() {
                        continue;
                    }
                    if let Some(file_name) = path.file_name().and_then(|n| n.to_str())
                        && let Some(caps) = bigtop_re.captures(file_name)?
                    {
                        let prefix = &caps[1];
                        let suffix = &caps[2];
                        let new_file_name = format!("{prefix}-{new_text_dash}{suffix}");
                        let new_path = path.with_file_name(new_file_name);
                        fs::rename(path, &new_path)?;
                    }
                }
            }
            let commit_message = if let Some(msg) = message {
                msg.clone()
            } else if is_version {
                format!("[Automated] Changed version from '{old_text}' to '{new_text}'")
            } else {
                format!("[Automated] Changed '{old_text}' to '{new_text}'")
            };

            // Commit and push the changes
            Command::new("git")
                .arg("add")
                .arg("-A")
                .current_dir(&repo_dir)
                .status()?;

            Command::new("git")
                .arg("commit")
                .arg("-a")
                .arg("-m")
                .arg(commit_message)
                .current_dir(&repo_dir)
                .status()?;

            if dry_run {
                return Ok(ModificationResult::DryRun(repo_dir));
            }

            let output = Command::new("git")
                .arg("push")
                .current_dir(&repo_dir)
                .output()?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if stdout.contains("Everything up-to-date") || stderr.contains("Everything up-to-date")
            {
                return Ok(ModificationResult::NoChanges);
            } else if !output.status.success() {
                return Err(GitError::GitPushError(format!(
                    "{repo}/{branch} '{}'",
                    stderr.trim()
                )));
            }

            Ok(ModificationResult::Success)
        })
        .await?;
        match result {
            Ok(m) => {
                match m {
                    ModificationResult::DryRun(p) => println!(
                        "ℹ️ {owner_slack}/{repo_slack}: Dry run - changes from '{old_text_slack}' to '{new_text_slack}' for branch '{branch_slack}' prepared in '{}' but not pushed",
                        p.display()
                    ),
                    ModificationResult::Success => {
                        let message = format!(
                            "{owner_slack}/{repo_slack}: Successfully changed {old_text_slack} to {new_text_slack} for branch {branch_slack}"
                        );
                        println!("✅ {message}");
                        self.append_slack_message(message).await;
                    }
                    ModificationResult::NoChanges => {
                        let message = format!(
                            "No changes to push in {owner_slack}/{repo_slack} for old text '{old_text_slack}' on branch '{branch_slack}'"
                        );
                        println!("{message}",);
                        self.append_slack_message(message).await;
                    }
                }
                Ok(())
            }
            Err(e) => {
                eprintln!(
                    "❌ Failed to change {old_text_slack} to {new_text_slack} for {owner_slack}/{repo_slack}: {e}"
                );
                self.append_slack_error(format!(
                    "{owner_slack}/{repo_slack}: Failed to change {old_text_slack} to {new_text_slack}: {e}"
                )).await;
                Err(e)
            }
        }
    }

    /// Change the release version for a specified branch across all configured repositories.
    /// This function clones each repository, updates the version, commits, and pushes the changes.
    /// Errors from each repository are collected and returned as a single error if any occur.
    #[allow(clippy::too_many_arguments)]
    pub async fn modify_all_branches(
        &self,
        branch: String,
        old_text: String,
        new_text: String,
        repositories: &[String],
        message: Option<String>,
        dry_run: bool,
        is_version: bool,
        quiet: bool,
    ) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            let branch = branch.clone();
            let old_text = old_text.clone();
            let new_text = new_text.clone();
            let message = message.clone();
            futures.push(async move {
                let result = self
                    .modify_branch(
                        repo.clone(),
                        branch,
                        old_text,
                        new_text,
                        message,
                        dry_run,
                        is_version,
                        quiet,
                    )
                    .await;
                (repo, result)
            });
        }

        // Collect errors from all repositories
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            if result.is_err() {
                errors.push((repo.clone(), result.err().unwrap()));
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(())
    }

    /// Get the name of all branches in a repository, optionally filtered by a regex
    pub async fn filter_branches<T, U>(&self, url: T, filter: U) -> Result<Vec<String>, GitError>
    where
        U: AsRef<str> + Display,
        T: AsRef<str> + ToString + Display,
    {
        let info = get_repo_info_from_url(url.as_ref())?;
        let (owner, repo) = (info.owner, info.repo_name);
        let all_branches: Vec<String> = self
            .fetch_branches(owner, repo)
            .await?
            .keys()
            .cloned()
            .collect();
        filter_ref(&all_branches, &filter)
    }

    /// Get a `HashMap` of Repository names -> Vec<branch names> for all repositories
    /// Optionally filtered by a regex
    pub async fn filter_all_branches<T, U>(
        &self,
        repositories: &[T],
        filter: U,
    ) -> Result<HashMap<String, Vec<String>>, GitError>
    where
        T: AsRef<str> + ToString + Display + Eq + Hash,
        U: AsRef<str> + Copy + Display,
    {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            futures.push(async move {
                let result = self.filter_branches(repo, filter).await;
                (repo.to_string(), result)
            });
        }
        let mut filtered_map: HashMap<String, Vec<String>> = HashMap::new();
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(branches) if !branches.is_empty() => {
                    filtered_map.insert(repo, branches);
                }
                // Don't add to the vector if no matching branches are returned
                Ok(_) => {}
                Err(e) => {
                    eprintln!("Failed to filter branches for {repo}: {e}");
                    errors.push((repo.clone(), e));
                }
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(filtered_map)
    }
}
