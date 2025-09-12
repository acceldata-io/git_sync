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

use crate::error::GitError;
use crate::github::client::{GithubClient, OutputMode};
use crate::utils::repo::http_to_ssh_repo;
use futures::stream::{FuturesUnordered, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use std::process::Command;
use std::sync::Arc;
use std::{path::Path, process::Stdio};

impl GithubClient {
    pub async fn backup_repo(&self, url: String, path: &Path) -> Result<(), GitError> {
        let semaphore_lock = self.semaphore.clone().acquire_owned().await?;
        let canonical_path = if let Ok(p) = path.canonicalize() {
            p
        } else {
            path.to_path_buf()
        };

        let ssh_url = http_to_ssh_repo(&url)?;

        let Some(name) = ssh_url.split('/').next_back() else {
            return Err(GitError::Other(format!(
                "Failed to extract repository name from URL: {ssh_url}"
            )));
        };

        let mut path = canonical_path.to_string_lossy().to_string();
        path.push('/');
        path.push_str(name);

        if let Some(t) = self.output.get() {
            if *t == OutputMode::Print && self.is_tty {
                println!("Backing up repository {url} to path: {path}");
            }
        }
        // If the mirrored repo already exists, update it rather than clone again
        let result = if Path::new(&path).exists() {
            tokio::task::spawn_blocking(move || {
                // If the path exists, we assume it's a git repository and we fetch the latest changes
                let output = Command::new("git")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .args(["remote", "update"])
                    .current_dir(&path)
                    .output();
                match output {
                    // This branch happens only if the output of the command was successful
                    Ok(s) if s.status.success() => Ok::<_, GitError>(format!(
                        "Successfully backed up repository: {ssh_url} to path: {path}"
                    )),
                    Ok(s) => {
                        let stderr = String::from_utf8_lossy(&s.stderr);
                        eprintln!("Error while backing up.");
                        eprintln!("STDERR:\n{stderr}");
                        Err(GitError::Other(format!(
                            "Failed to back up repository: {ssh_url}, git exited with status: {}",
                            s.status
                        )))
                    }
                    Err(e) => Err(GitError::Other(format!(
                        "Failed to back up repository: {ssh_url}, error: {e}"
                    ))),
                }
            })
            .await?
        // Mirror the repo
        } else {
            tokio::task::spawn_blocking(move || {
                // Clone the reposittory as a mirror
                let output = Command::new("git")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .args(["clone", "--mirror", &ssh_url, &path])
                    .output();
                match output {
                    // This branch happens only if the output of the command was successful
                    Ok(s) if s.status.success() => Ok::<_, GitError>(format!(
                        "Successfully backed up repository: {ssh_url} to path: {path}"
                    )),
                    Ok(s) => {
                        let stderr = String::from_utf8_lossy(&s.stderr);
                        eprintln!("Error while backing up.");
                        eprintln!("STDERR:\n{stderr}");
                        Err(GitError::Other(format!(
                            "Failed to back up repository: {ssh_url}, git exited with status: {}",
                            s.status
                        )))
                    }
                    Err(e) => Err(GitError::Other(format!(
                        "Failed to back up repository: {ssh_url}, error: {e}"
                    ))),
                }
            })
            .await?
        };

        // Explicitly drop the lock here
        drop(semaphore_lock);

        // Match outside of the spawn_blocking to avoid ownership issues
        match result {
            Ok(m) => {
                if let Some(t) = self.output.get() {
                    if *t == OutputMode::Print && self.is_tty {
                        println!("{m}");
                    }
                }
                self.append_slack_message(m).await;
            }
            Err(e) => {
                if let Some(t) = self.output.get() {
                    if *t == OutputMode::Print && self.is_tty {
                        eprintln!("{e}");
                    }
                }
                self.append_slack_error(e.to_string()).await;
                return Err(e);
            }
        }

        Ok(())
    }
    pub async fn backup_all_repos(
        &self,
        repositories: Vec<String>,
        path: &Path,
    ) -> Result<(), GitError> {
        let mut futures = FuturesUnordered::new();
        let number_of_repos = repositories.len();

        if self.is_tty && number_of_repos > 1 {
            let _ = self.output.set(OutputMode::Progress);
        }

        let progress = if self.is_tty {
            let pb = ProgressBar::new(number_of_repos as u64);
            pb.set_style(
                ProgressStyle::with_template(
                    "[{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
                )
                .unwrap()
                .progress_chars("#>-"),
            );
            pb.enable_steady_tick(std::time::Duration::from_millis(100));
            Some(Arc::new(pb))
        } else {
            None
        };

        if let Some(ref progress) = progress {
            progress.println(format!("Backing up all configured repositories..."));
        }

        for repo in repositories {
            let progress = progress.clone();
            futures.push(async move {
                let result = self.backup_repo(repo.clone(), path).await;
                (result, repo)
            });
        }

        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((result, repo)) = futures.next().await {
            if let Some(ref progress) = progress {
                progress.inc(1);
            }
            match result {
                Ok(()) => {
                    self.append_slack_message(format!("Backed up repository: {repo}"))
                        .await;

                    if let Some(ref progress) = progress {
                        progress.println(format!("Backed up repository: {repo}"));
                    }
                }
                Err(e) => {
                    self.append_slack_error(format!(
                        "Failed to back up repository: {repo}, error: {e}"
                    ))
                    .await;

                    if let Some(ref progress) = progress {
                        progress.println(format!("Failed to back up repository: {repo}"));
                    }
                    errors.push((repo, e));
                }
            }
        }
        if let Some(progress) = progress {
            progress.finish();
        }
        Ok(())
    }
    #[cfg(feature = "aws")]
    pub fn backup_to_s3(
        &self,
        ssh_url: &str,
        file_path: &Path,
        bucket: &str,
        key: &str,
    ) -> Result<(), GitError> {
        Ok(())
    }
}
