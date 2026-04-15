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
use crate::error::GitError;
use crate::error::is_retryable;
use crate::github::client::GithubClient;
use crate::utils::file_utils::{replace_all_in_directory, replace_all_in_directory_with};
use crate::utils::filter::{filter_ref, get_or_compile};
use crate::utils::repo::{async_retry, get_repo_info_from_url, get_sha_from_ref, http_to_ssh_repo};
use fancy_regex::Captures;
use futures::{StreamExt, stream::FuturesUnordered};
use http_body_util::BodyExt;
use log::debug;
use octocrab::models::repos::Object;
use octocrab::params::repos::Reference;
use std::fmt::Display;
use std::fs;
use std::process::Command;
use tempfile::TempDir;
use walkdir::WalkDir;

use std::collections::HashMap;
use std::fs::File;
use std::hash::Hash;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

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

        let sha = get_sha_from_ref(
            &self.octocrab,
            &self.semaphore,
            &owner,
            &repo,
            Reference::Branch(base_branch.to_string()),
        )
        .await?;

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

        let res = async_retry(100, 5000, 3, GitError::is_retryable, || {
            let owner = owner.clone();
            let repo = repo.clone();
            let reference = Reference::Tag(base_tag.to_string());
            async move {
                get_sha_from_ref(&self.octocrab, &self.semaphore, &owner, &repo, reference).await
            }
        })
        .await;
        let response = match res {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Unable to create branch from tag: {e}");
                self.append_slack_error(format!(
                    "{owner}/{repo}: Unable to fetch tag {base_tag} from Github: {e}"
                ))
                .await;
                return Err(e);
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

        let res: Result<_, GitError> = async_retry(100, 5000, 3, GitError::is_retryable, || {
            let owner = owner.clone();
            let repo = repo.clone();
            let new_branch = new_branch.as_ref().to_string();
            let octocrab = self.octocrab.clone();
            let response = response.clone();
            async move {
                let _permit = self.semaphore.clone().acquire_owned().await;
                octocrab
                    .repos(&owner, &repo)
                    .create_ref(&Reference::Branch(new_branch), response)
                    .await
                    .map_err(GitError::from)
            }
        })
        .await;
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
                    GitError::GithubApiError(octocrab::Error::GitHub { source, .. }) => {
                        eprintln!("{source:#?}");
                        return Ok(());
                    }
                    _ => eprintln!("{e}"),
                }
                Err(e)
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

        let result: Result<_, octocrab::Error> = async_retry(
            100,
            5000,
            3,
            |e: &octocrab::Error| is_retryable(e),
            || {
                let owner = owner.clone();
                let repo = repo.clone();
                let branch = branch.as_ref();
                async move {
                    let _permit = self.semaphore.clone().acquire_owned().await;
                    self.octocrab
                        .clone()
                        .repos(&owner, &repo)
                        .delete_ref(&Reference::Branch(branch.to_string()))
                        .await
                }
            },
        )
        .await;

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
                .expect("Unable to compile regex using '{old_text}'");
            replace_all_in_directory(&repo_dir, &re, new_text.as_str(), quiet);

            if repo == "odp-bigtop" && is_version {
                // Replace some special cases in bigtop
                replace_odp_and_bn(&old_text, &new_text, &repo_dir, quiet)?;
                // If we're working with the odp-bigtop repo, we also need to rename package files
                // in bigtop-packages/src/deb
                fix_debian_paths(&old_text, &new_text, &repo_dir, quiet)?;
                // Fix hadoop jar versions listed in any files
                fix_hadoop_jars(
                    &old_text,
                    &new_text,
                    &repo_dir.join("bigtop-packages/src/common/hadoop"),
                    quiet,
                )?;
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
        debug!(
            "Finished filtering. Available permits are now: {}",
            self.semaphore.available_permits()
        );
        filter_ref(&all_branches, &filter)
    }
    /// Check to see if a branch is present in a repository
    pub async fn is_branch_present<T, U>(&self, url: T, branch: U) -> Result<bool, GitError>
    where
        T: AsRef<str> + Display,
        U: AsRef<str> + Display,
    {
        let info = get_repo_info_from_url(&url)?;
        let (owner, repository) = (info.owner, info.repo_name);
        let branches: Vec<String> = self
            .fetch_branches(owner, repository)
            .await?
            .keys()
            .cloned()
            .collect();
        Ok(branches.iter().any(|b| b == branch.as_ref()))
    }
    /// Check to see if a branch is *not* present, in all passed repositories
    pub async fn is_branch_present_all<T, U>(
        &self,
        repositories: &[T],
        branch: U,
    ) -> Result<HashMap<String, bool>, GitError>
    where
        T: AsRef<str> + ToString + Display + Eq + Hash,
        U: AsRef<str> + Display,
    {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            let branch = branch.as_ref();
            futures.push(async move {
                let result = self.is_branch_present(repo, branch).await;
                (repo.to_string(), result)
            });
        }
        let mut presence_map: HashMap<String, bool> = HashMap::new();
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(is_present) => {
                    presence_map.insert(repo, is_present);
                }
                Err(e) => {
                    eprintln!("Failed to check branch presence for {repo}: {e}");
                    errors.push((repo.clone(), e));
                }
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(presence_map)
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
            debug!("Processing results for {repo}");
            match result {
                Ok(branches) if !branches.is_empty() => {
                    debug!("{repo} succeeded");
                    filtered_map.insert(repo, branches);
                }
                // Don't add to the vector if no matching branches are returned
                Ok(_) => {
                    debug!("No matching branches for {repo}, skipping");
                }
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
    /// Download branches for a single repository, based off of a specified branch and/or a regex filter.
    pub async fn download_branches<T, U, V, W>(
        &self,
        url: T,
        branch: Option<U>,
        regex_filter: V,
        output_dir: &Path,
        prefix: W,
    ) -> Result<(), GitError>
    where
        T: AsRef<str> + Display,
        U: AsRef<str> + Display,
        V: AsRef<str> + Display,
        W: AsRef<str> + Display,
    {
        if !Path::new(output_dir).exists() {
            fs::create_dir_all(output_dir)?;
        }

        let info = get_repo_info_from_url(&url)?;
        let (owner, repo) = (info.owner, info.repo_name);

        let mut refs: Vec<String> = Vec::new();
        if !regex_filter.as_ref().is_empty() {
            self.filter_branches(url, regex_filter)
                .await?
                .iter()
                .for_each(|b| refs.push(b.clone()));
        }
        if let Some(b) = branch {
            refs.push(b.as_ref().to_string());
        }
        let octocrab = self.octocrab.clone();
        for branch in refs {
            let permit = self.semaphore.clone().acquire_owned().await?;
            let mut tarball_name = format!("{branch}.tar.gz");
            // This helps prevent collisions between tarball names from different repositories
            if tarball_name.starts_with(prefix.as_ref()) && !prefix.as_ref().is_empty() {
                tarball_name = format!("{repo}-{tarball_name}");
            }

            let mut resp = octocrab
                .repos(&owner, &repo)
                .download_tarball(Reference::Branch(branch.clone()))
                .await?;
            let body = resp.body_mut();

            let file_path = output_dir.join(tarball_name);
            let mut file = File::create(&file_path)?;
            while let Some(next) = body.frame().await {
                let frame = next?;
                if let Some(chunk) = frame.data_ref() {
                    file.write_all(chunk)?;
                }
            }
            drop(permit);
        }
        Ok(())
    }
    /// Download branches for all selected repositories, based off of a specified branch and/or a
    /// regex filter.
    pub async fn download_all_branches<T, U, V>(
        &self,
        repositories: &[T],
        branch: Option<U>,
        regex_filter: V,
        output_dir: &Path,
        prefix: String,
    ) -> Result<(), GitError>
    where
        T: AsRef<str> + Display,
        U: AsRef<str> + Display,
        V: AsRef<str> + Display,
    {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            let branch = branch.as_ref();
            let regex_filter = regex_filter.as_ref();
            let prefix = prefix.clone();
            futures.push(async move {
                let result = self
                    .download_branches(repo, branch, regex_filter, output_dir, &prefix)
                    .await;
                (repo.to_string(), result)
            });
        }
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((repo, result)) = futures.next().await {
            match result {
                Ok(()) => {
                    if self.is_tty {
                        println!(
                            "✅ Successfully downloaded branches for {repo} to '{}'",
                            output_dir.display()
                        );
                    }
                }
                Err(e) => {
                    eprintln!("❌ Failed to download branches for {repo}: {e}");
                    errors.push((repo.clone(), e));
                }
            }
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(())
    }
}
/// This specifically targets odp-bigtop, in order to set the build number and
/// odp version correctly.
/// Other occurences of the ODP version work fine, such as 'ref' in bigtop.bom
/// but are not handled here
fn replace_odp_and_bn(
    old_text: &str,
    new_text: &str,
    repo_dir: &Path,
    quiet: bool,
) -> Result<(), GitError> {
    let is_odp = get_or_compile(
        r"(?P<odp>[0-9]+[.][0-9]+[.][0-9]+[.][0-9]+)-(?P<build_num>[0-9]+|SNAPSHOT)",
    )
    .expect("Unable to compile ODP regex");
    if let Some(captures) = is_odp.captures(old_text)? {
        let odp_version = match captures.name("odp") {
            Some(m) => m.as_str().to_string(),
            None => {
                return Err(GitError::Other(
                    "Some error occurred while determining the ODP VERSION".to_string(),
                ));
            }
        };

        let escaped_odp_version = fancy_regex::escape(&odp_version);

        let odp_regex_text = format!(
            r#"^(?<indent>\s*)(?<key>version|VERSION|ODP_VERSION|odp_version)(?![A-Za-z0-9_])\s*=\s*\"?{escaped_odp_version}"?"#
        );
        let odp_version_re = get_or_compile(odp_regex_text)?;
        let new_odp_version = new_text.split('-').next().ok_or_else(|| {
            GitError::Other("Some error occurred while determining the ODP version".to_string())
        })?;

        let replacement = |captures: &Captures| {
            format!(
                r#"{}{} = "{}""#,
                captures.name("indent").map(|m| m.as_str()).unwrap_or(""),
                captures
                    .name("key")
                    .expect("Could not get ODP version")
                    .as_str(),
                new_odp_version
            )
        };
        replace_all_in_directory_with(repo_dir, &odp_version_re, &replacement, quiet);
    }
    let build_number = new_text
        .split('-')
        .nth(1)
        .ok_or_else(|| GitError::Other("Could not determine new build number".to_string()))?;

    let bn_regex_string = r#"(?<indent>\s*)(?<bn>odp_bn|ODP_BN)\s*=\s*"(?:[0-9]+|SNAPSHOT)";"#;
    let bn_regex = get_or_compile(bn_regex_string)
        .map_err(|e| GitError::Other(format!("Unable to compile ODP build number regex: {e}")))?;

    // This grabs the capture group, which will be either ODP_BN or odp_bn, then uses
    // a closure to correctly format the replacement string.
    replace_all_in_directory_with(
        repo_dir,
        &bn_regex,
        &|caps: &Captures| {
            format!(
                r#"{}{} = "{}";"#,
                &caps.name("indent").map(|m| m.as_str()).unwrap_or(""),
                &caps
                    .name("bn")
                    .ok_or_else(|| GitError::Other("Could not get ODP build number".to_string()))
                    // It's better if we crash the whole program here instead of potentially
                    // introducing unexpected results in the git repo
                    .expect("Failed to get build num")
                    .as_str(),
                build_number
            )
        },
        quiet,
    );
    Ok(())
}
/// Some debian files have the odp version embedded into the name of the file,
/// as well as in the name of the package, so we need to rename the file and
/// replace the version in the file if it's present.
fn fix_debian_paths(
    old_text: &str,
    new_text: &str,
    repo_dir: &Path,
    quiet: bool,
) -> Result<(), GitError> {
    let dir = format!("{}/bigtop-packages/src/deb", repo_dir.display());

    let old_text_dash = old_text.replace('.', "-");
    let new_text_dash = new_text.replace('.', "-");

    let odp_version_dashes_re =
        get_or_compile(fancy_regex::escape(&old_text_dash)).map_err(|e| {
            GitError::Other(format!(
                "Unable to compile ODP version with dashes regex using '{old_text_dash}': {e}"
            ))
        })?;

    replace_all_in_directory(repo_dir, &odp_version_dashes_re, &new_text_dash, quiet);

    let bigtop_re_text = format!("^(.*)-{}([.-].*)(.*)", fancy_regex::escape(&old_text_dash));

    let bigtop_re = get_or_compile(&bigtop_re_text).map_err(|e| {
        GitError::Other(format!(
            "Failed to compile bigtop regex '{bigtop_re_text}': {e}"
        ))
    })?;

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
    Ok(())
}
/// This can only be used when the odp version and the hadoop version can match
/// So 13.3.6 and 3.3.6.2-1 will not match
/// This function only matters when the major/minor hadoop version has been modified
fn fix_hadoop_jars(
    old_text: &str,
    new_text: &str,
    path: &Path,
    quiet: bool,
) -> Result<(), GitError> {
    // If this isn't an odp version, we can skip this. This makes no sense otherwise
    let version_re = get_or_compile(r"^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+-([0-9]+|SNAPSHOT)$")?;
    if !version_re.is_match(old_text)? || !version_re.is_match(new_text)? {
        return Ok(());
    }
    let hadoop_version =
        |version: &str| -> String { version.splitn(4, '.').take(3).collect::<Vec<_>>().join(".") };

    // The odp version will already have been modified once we hit this point which is why we match
    // it against old_text, new_text
    let old_full_version = format!("{}.{}", hadoop_version(old_text), new_text);
    let new_full_version = format!("{}.{}", hadoop_version(new_text), new_text);

    let escaped = fancy_regex::escape(&old_full_version);
    let old_text_re = get_or_compile(format!(r"\b{escaped}\b")).unwrap_or_else(|e| {
        panic!("Unable to compile regex for Hadoop JARs using '{old_text}': {e}")
    });
    replace_all_in_directory(path, &old_text_re, &new_full_version, quiet);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    /// If using this function, make sure to hold onto the `TempDir` reference
    /// because once it goes out of scope, that temp directory is deleted.
    fn create_test_file(filename: &str, content: &str) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join(filename);
        fs::write(&file_path, content).unwrap();
        (dir, file_path)
    }
    #[test]
    /// Checks that replace_odp_and_bn correctly updates odp version and build number,
    /// including SNAPSHOT → numeric build number transformation.
    fn replace_odp_and_bn_updates_version_and_build_number() {
        let input = r#"stack {
    'odp' {
        version = "3.3.6.3";
        odp_version = "3.3.6.3";
        odp_bn = "SNAPSHOT";
    }
    'ODP' {
        VERSION = "3.3.6.3";
        ODP_VERSION = "3.3.6.3";
        ODP_BN = "SNAPSHOT";
        ODP_VERSION_AS_NAME = ODP_VERSION.replaceAll("\\.", "-") + "-" + ODP_BN;
    }
}\n\n\n"#;
        let expected_output = r#"stack {
    'odp' {
        version = "3.3.6.4";
        odp_version = "3.3.6.4";
        odp_bn = "1";
    }
    'ODP' {
        VERSION = "3.3.6.4";
        ODP_VERSION = "3.3.6.4";
        ODP_BN = "1";
        ODP_VERSION_AS_NAME = ODP_VERSION.replaceAll("\\.", "-") + "-" + ODP_BN;
    }
}\n\n\n"#;

        let (_dir, file) = create_test_file("test.txt", input);
        let dir = file.parent().unwrap();

        replace_odp_and_bn("3.3.6.3-SNAPSHOT", "3.3.6.4-1", dir, true).unwrap();

        let got = fs::read_to_string(&file).unwrap();
        assert_eq!(got, expected_output);
    }

    #[test]
    /// Checks that fix_debian_paths correctly updates file contents and renames
    /// files containing the old ODP version in the deb directory.
    fn fix_debian_paths_updates_content_and_renames_files() {
        let dir = TempDir::new().unwrap();
        let deb_dir = dir.path().join("bigtop-packages/src/deb/kudu/debian");
        fs::create_dir_all(&deb_dir).unwrap();

        let control_path = deb_dir.join("control");
        fs::write(&control_path, "Package: kudu-3-3-6-3-SNAPSHOT\n").unwrap();

        let install_path = deb_dir.join("kudu-3-3-6-3-SNAPSHOT.install");
        fs::write(&install_path, "").unwrap();

        fix_debian_paths("3.3.6.3-SNAPSHOT", "3.3.6.4-1", dir.path(), true).unwrap();

        let control = fs::read_to_string(&control_path).unwrap();
        assert!(
            control.contains("kudu-3-3-6-4-1"),
            "control content not updated: {control}"
        );
        assert!(
            !control.contains("3-3-6-3-SNAPSHOT"),
            "old version still in control: {control}"
        );

        assert!(!install_path.exists(), "old .install file still exists");
        assert!(
            deb_dir.join("kudu-3-3-6-4-1.install").exists(),
            "renamed .install file not found"
        );
    }
    #[test]
    /// Checks that fix_debian_paths does nothing when it can't find matches
    fn debian_rename_fails() {
        let dir = TempDir::new().unwrap();
        let deb_dir = dir.path().join("bigtop-packages/src/deb/kudu/debian");
        fs::create_dir_all(&deb_dir).unwrap();

        let control_path = deb_dir.join("control");
        fs::write(&control_path, "Package: kudu-3-3-6-3-SNAPSHOT\n").unwrap();

        let install_path = deb_dir.join("kudu-3-3-6-3-SNAPSHOT.install");
        fs::write(&install_path, "").unwrap();

        fix_debian_paths("3.3.6.3-101", "3.3.6.4-1", dir.path(), true).unwrap();

        let control = fs::read_to_string(&control_path).unwrap();

        assert!(
            control.contains("3-3-6-3-SNAPSHOT"),
            "Control content modified: {control}"
        );

        assert!(install_path.exists(), "old .install file deleted");
        assert!(
            deb_dir.join("kudu-3-3-6-3-SNAPSHOT.install").exists(),
            "Original .install file deleted"
        );
    }
    #[test]
    fn fix_hadoop_jar_version() {
        // This input came from an actual file in odp-bigtop, with slightly
        // modified version numbers for the test
        let input = r#"
hadoop-annotations-3.3.6.3.3.7.1-1.jar
hadoop-auth-3.3.6.3.3.7.1-1.jar
hadoop-common-3.3.6.3.3.7.1-1.jar
hadoop-hdfs-client-3.3.6.3.3.7.1-1.jar
hadoop-mapreduce-client-common-3.3.6.3.3.7.1-1.jar
hadoop-mapreduce-client-core-3.3.6.3.3.7.1-1.jar
hadoop-mapreduce-client-jobclient-3.3.6.3.3.7.1-1.jar
hadoop-yarn-api-3.3.6.3.3.7.1-1.jar
hadoop-yarn-client-3.3.6.3.3.7.1-1.jar
hadoop-yarn-common-3.3.6.3.3.7.1-1.jar
"#;
        let expected_output = r#"
hadoop-annotations-3.3.7.3.3.7.1-1.jar
hadoop-auth-3.3.7.3.3.7.1-1.jar
hadoop-common-3.3.7.3.3.7.1-1.jar
hadoop-hdfs-client-3.3.7.3.3.7.1-1.jar
hadoop-mapreduce-client-common-3.3.7.3.3.7.1-1.jar
hadoop-mapreduce-client-core-3.3.7.3.3.7.1-1.jar
hadoop-mapreduce-client-jobclient-3.3.7.3.3.7.1-1.jar
hadoop-yarn-api-3.3.7.3.3.7.1-1.jar
hadoop-yarn-client-3.3.7.3.3.7.1-1.jar
hadoop-yarn-common-3.3.7.3.3.7.1-1.jar
"#;
        let (_dir, file) = create_test_file("test.txt", input);
        let dir = file.parent().unwrap();

        fix_hadoop_jars("3.3.6.4-1", "3.3.7.1-1", dir, true).unwrap();

        let got = fs::read_to_string(&file).unwrap();
        assert_eq!(got, expected_output);
    }
    #[test]
    /// Make sure that if there is no match, it does nothing
    fn hadoop_jar_versions_no_match() {
        let input = r#"abc\ndef\nghi"#;
        let expected_output = r#"abc\ndef\nghi"#;
        let (_dir, file) = create_test_file("test.txt", input);
        let dir = file.parent().unwrap();
        fix_hadoop_jars("def", "my_new_text", dir, true).unwrap();
        let got = fs::read_to_string(&file).unwrap();
        assert_eq!(got, expected_output);
    }
    #[test]
    /// Test that partial matches (like ^13.3.6) don't match
    fn fix_hadoop_jars_no_partial_match() {
        let input = "hadoop-annotations-13.3.6.3.3.7.1-1.jar\n";
        let (_dir, file) = create_test_file("hadoop-client.list", input);
        let dir = file.parent().unwrap();
        fix_hadoop_jars("3.3.6.3-SNAPSHOT", "3.3.7.1-1", dir, true).unwrap();
        let got = fs::read_to_string(&file).unwrap();
        assert_eq!(
            got, input,
            "partial match should not have been replaced: {got}"
        );
    }
}
