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
use crate::utils::file_replace::replace_all_in_directory;
use crate::utils::repo::{get_repo_info_from_url, http_to_ssh_repo};
use fancy_regex::Regex;
use futures::{StreamExt, stream::FuturesUnordered};
use octocrab::models::repos::Object;
use octocrab::params::repos::Reference;
use std::fmt::Display;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use temp_dir::TempDir;
use walkdir::WalkDir;

use std::sync::OnceLock;

/// Initialized once, then it becomes available
/// from then on so we don't have to compile our regex every
/// single time `change_release_version` is called
static REPO_REGEX: OnceLock<Regex> = OnceLock::new();

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

enum TmpDirHolder {
    DryRun(PathBuf),
    Temp(TempDir),
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
    pub async fn change_release_version(
        &self,
        url: String,
        branch: String,
        old_version: String,
        new_version: String,
        message: Option<String>,
        dry_run: bool,
    ) -> Result<(), GitError> {
        let info = get_repo_info_from_url(url)?;
        let (repo, owner, url) = (info.repo_name, info.owner, info.url);
        let repo_copy = repo.clone();
        let old_version_copy = old_version.clone();
        let new_version_copy = new_version.clone();
        let owner_copy = owner.clone();
        let branch_copy = branch.clone();

        // Acquire a lock on the semaphore to limit concurrent operations
        let permit = self.semaphore.clone().acquire_owned().await?;

        let url = http_to_ssh_repo(&url)?;

        let result = tokio::task::spawn_blocking(move || {
            let _lock = permit;
            let tmp_holder;
            if dry_run {
                let tmp_dir = PathBuf::from(format!("/tmp/{branch}"));
                fs::create_dir_all(&tmp_dir)?;
                tmp_holder = TmpDirHolder::DryRun(tmp_dir);
            } else {
                let tmp_dir = TempDir::new()
                    .map_err(|e| GitError::Other(format!("Failed to create temp dir: {e}")))?;
                tmp_holder = TmpDirHolder::Temp(tmp_dir);
            }
            let tmp = match &tmp_holder {
                TmpDirHolder::DryRun(p) => p.as_path(),
                TmpDirHolder::Temp(t) => t.path(),
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
                .current_dir(tmp)
                .status()?;

            if git_status.success() {
                println!("Cloned {repo}'s branch {branch} into {}", tmp.display());
            } else {
                return Err(GitError::Other(format!(
                    "Failed to clone {repo}'s branch {branch} into {}",
                    tmp.display()
                )));
            }

            let re = REPO_REGEX.get_or_init(|| {
                let msg = format!("Invalid regex for changing version '{old_version}'");
                Regex::new(&fancy_regex::escape(old_version.as_ref())).expect(&msg)
            });
            replace_all_in_directory(&repo_dir, re, new_version.as_str());
            // If we're working with the odp-bigtop repo, we also need to rename package files
            // in bigtop-packages/src/deb
            if repo == "odp-bigtop" {
                let build_number = new_version
                    .split('-')
                    .last()
                    .ok_or_else(|| GitError::Other("Invalid new version format".to_string()))?;

                replace_all_in_directory(
                    &repo_dir,
                    &Regex::new(r#"odp_bn\s*=\s*"\d+";"#).expect("Invalid odp_bn regex"),
                    &format!(r#"odp_bn = "{}";"#, build_number),
                );
                replace_all_in_directory(
                    &repo_dir,
                    &Regex::new(r#"ODP_BN\s*=\s*"\d+";"#).expect("Invalid ODP_BN regex"),
                    &format!(r#"ODP_BN = "{}";"#, build_number),
                );

                let dir = format!("{}/bigtop-packages/src/deb", repo_dir.display());

                let old_version_dash = &old_version.to_string().replace('.', "-");
                let new_version_dash = &new_version.to_string().replace('.', "-");
                println!("{old_version_dash}");
                println!("{new_version_dash}");

                let bigtop_re = Regex::new(&format!(
                    r"^(.*)-{}([.-].*)(.*)$",
                    fancy_regex::escape(old_version_dash)
                ))
                .expect("Failed to compile old version regex");

                for entry in WalkDir::new(&dir)
                    .into_iter()
                    .filter_map(Result::ok)
                {
                    let path = entry.path();
                    if path.is_dir() {
                        continue;
                    }
                    if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                        println!("File name is {file_name}");
                        if let Some(caps) = bigtop_re.captures(file_name)?
                        {
                            println!("Modifying file names...");
                            let prefix = &caps[1];
                            let suffix = &caps[2];
                            println!("Prefix: {prefix}, Suffix: {suffix}");
                            let new_file_name = format!("{prefix}-{new_version_dash}{suffix}");
                            println!("New file name: {new_file_name}");
                            let new_path = path.with_file_name(new_file_name);
                            println!("New path: {}", new_path.display());
                            fs::rename(path, &new_path)?;
                        }
                    }
                }
            }
            let commit_message = if let Some(msg) = message {
                msg.to_string()
            } else {
                format!("[Automated] Changed version from {old_version} to {new_version}")
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

            if !dry_run {
                let output = Command::new("git")
                    .arg("push")
                    .current_dir(&repo_dir)
                    .output()?;
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                let message = format!("No changes to push in {repo}/{owner} for old version '{old_version}' on branch {branch}");

                if stdout.contains("Everything up-to-date") || stderr.contains("Everything up-to-date") {
                    return Ok(message);
                } else if !output.status.success() {
                    return Err(GitError::Other(format!(
                        "Failed to push changes to {repo} on branch {branch}: {}",
                        stderr.trim()
                    )));
                }
            }

            Ok(String::new())
        })
        .await?;
        match result {
            Ok(m) => {
                if m.is_empty() {
                    let message = format!(
                        "{repo_copy}/{owner_copy}: Successfully changed version from {old_version_copy} to {new_version_copy} for branch {branch_copy}"
                    );
                    println!("✅ {message}");
                    self.append_slack_message(message).await;
                } else {
                    println!("✅ {m}");
                    self.append_slack_message(m).await;
                }
                Ok(())
            }
            Err(e) => {
                eprintln!(
                    "❌ Failed to change version from {old_version_copy} to {new_version_copy} for {repo_copy}/{owner_copy}: {e}"
                );
                self.append_slack_error(format!(
                    "{repo_copy}/{owner_copy}: Failed to change version from {old_version_copy} to {new_version_copy}: {e}"
                )).await;
                Err(e)
            }
        }
    }
    /// Change the release version for a specified branch across all configured repositories.
    /// This function clones each repository, updates the version, commits, and pushes the changes.
    /// Errors from each repository are collected and returned as a single error if any occur.
    pub async fn change_all_release_version(
        &self,
        branch: String,
        old_version: String,
        new_version: String,
        repositories: &[String],
        message: Option<String>,
        dry_run: bool,
    ) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            let branch = branch.to_string();
            let old_version = old_version.to_string();
            let new_version = new_version.to_string();
            let message = message.clone();
            futures.push(async move {
                let result = self
                    .change_release_version(
                        repo.to_string(),
                        branch,
                        old_version,
                        new_version,
                        message,
                        dry_run,
                    )
                    .await;
                (repo, result)
            });
        }

        // Collect errors from all repositories
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            if result.is_err() {
                errors.push((repo.to_string(), result.err().unwrap()));
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(())
    }
}
