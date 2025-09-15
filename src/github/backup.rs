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

use tokio::fs::File;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::task::JoinHandle;

#[cfg(feature = "aws")]
use aws_config::{BehaviorVersion, meta::region::RegionProviderChain};
#[cfg(feature = "aws")]
use aws_sdk_s3::{
    Client,
    types::{CompletedMultipartUpload, CompletedPart},
};

const PART_SIZE: usize = 64 * 1024 * 1024;

const MIN_SIZE: usize = 5 * 1024 * 1024;
impl GithubClient {
    #[allow(too_many_lines)]
    pub async fn backup_repo(&self, url: String, path: &Path) -> Result<(), GitError> {
        let semaphore_lock = Arc::clone(&self.semaphore).acquire_owned().await?;
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
                let _lock = semaphore_lock;
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
                let _lock = semaphore_lock;
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
                        if Path::new(&path).exists() {
                            // If the clone failed, remove the directory to avoid leaving a broken repo
                            std::fs::remove_dir_all(&path)?;
                        }
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
            progress.println("Backing up all configured repositories...");
        }

        for repo in repositories {
            //let progress = progress.clone();
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
    pub async fn prune_backup(&self, path: &Path, since: Option<String>) -> Result<(), GitError> {
        let ext = match path.extension() {
            Some(ext) => ext.to_str().unwrap_or(""),
            _ => return Err(GitError::Other("Failed to get file extension".to_string())),
        };

        if !path.exists() {
            return Err(GitError::Other(format!(
                "Path {} does not exist",
                path.display()
            )));
        }
        if ext != "git" {
            return Err(GitError::Other(format!(
                "Path {} is not a git repository",
                path.display()
            )));
        }
        let path = path.to_path_buf();
        let result = tokio::task::spawn_blocking(move || {
            let output = if let Some(since) = since {
                Command::new("git")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .args(["gc", &format!("--prune={since}")])
                    .current_dir(&path)
                    .output()
            } else {
                Command::new("git")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .args(["gc", "--prune=now"])
                    .current_dir(&path)
                    .output()
            };

            match output {
                // This branch happens only if the output of the command was successful
                Ok(s) if s.status.success() => Ok::<_, GitError>(format!(
                    "Successfully pruned backup at path: {}",
                    path.display()
                )),
                Ok(s) => {
                    let stderr = String::from_utf8_lossy(&s.stderr);
                    eprintln!("Error while pruning backup.");
                    eprintln!("STDERR:\n{stderr}");
                    Err(GitError::Other(format!(
                        "Failed to prune backup at path: {}, git exited with status: {}",
                        path.display(),
                        s.status
                    )))
                }
                Err(e) => Err(GitError::Other(format!(
                    "Failed to prune backup at path: {}, error: {e}",
                    path.display()
                ))),
            }
        })
        .await?;

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
    #[cfg(feature = "aws")]
    /// Upload a backup to s3. This requires your aws credentials to be configured in the
    /// environment
    #[allow(too_many_lines)]
    pub async fn backup_to_s3(
        &self,
        file_path: &Path,
        bucket: &str,
        key: &str,
    ) -> Result<(), GitError> {
        let region = RegionProviderChain::default_provider().or_else("ca-central-1");

        // Use the behavour version from when this was developped
        let config = aws_config::defaults(BehaviorVersion::v2025_08_07())
            .region(region)
            .load()
            .await;

        let client = Arc::new(Client::new(&config));

        let mut file = File::open(file_path).await?;
        file.set_max_buf_size(PART_SIZE);
        let mut file_buffer = BufReader::new(file);
        let mut part_number = 1;

        // If the file is smaller than the minimum size allowed for multipart uploads, just upload
        // the whole thing
        let size = file_buffer.get_ref().metadata().await?.len();
        if size == 0 {
            return Err(GitError::Other("Backup file is empty".to_string()));
        }

        if size < PART_SIZE as u64 {
            let body = aws_sdk_s3::primitives::ByteStream::from_path(file_path)
                .await
                .map_err(|e| GitError::Other(e.to_string()))?;
            client
                .put_object()
                .bucket(bucket)
                .key(key)
                .content_type("application/gzip")
                .body(body)
                .send()
                .await
                .map_err(|e| GitError::AWSError(e.to_string()))?;
            return Ok(());
        }

        // Initiate the multipart upload
        let response = client
            .create_multipart_upload()
            .bucket(bucket)
            .key(key)
            .content_type("application/gzip")
            .send()
            .await
            .map_err(|e| GitError::AWSError(e.to_string()))?;

        let upload_id = response
            .upload_id()
            .ok_or_else(|| GitError::AWSError("Failed to get upload ID".to_string()))?;

        let mut futures: FuturesUnordered<JoinHandle<Result<(i32, String), _>>> =
            FuturesUnordered::new();

        let mut results: Vec<(i32, String)> = Vec::new();

        loop {
            let mut buffer = vec![0u8; PART_SIZE];

            let n = file_buffer.read(&mut buffer).await?;
            if n == 0 {
                break;
            }

            buffer.truncate(n);
            let client = Arc::clone(&client);
            let bucket = bucket.to_string();
            let key = key.to_string();
            let upload_id = upload_id.to_string();
            let local_part_number = part_number;

            let lock = Arc::clone(&self.semaphore).acquire_owned().await?;
            println!("Starting upload of part {local_part_number}");
            futures.push(tokio::spawn(async move {
                let _lock = lock;
                let response = client
                    .upload_part()
                    .bucket(&bucket)
                    .key(&key)
                    .upload_id(&upload_id)
                    .part_number(local_part_number)
                    .body(buffer.into())
                    .send()
                    .await
                    .map_err(|e| GitError::AWSError(e.to_string()))?;
                let etag = response.e_tag().unwrap().to_string();
                println!("Uploaded part {local_part_number}");

                Ok::<(i32, String), GitError>((local_part_number, etag))
            }));
            part_number += 1;
        }
        while let Some(res) = futures.next().await {
            match res {
                Ok(r) => results.push(r?),
                Err(e) => {
                    return Err(GitError::Other(format!(
                        "Failure when uploading multipart file: {e}"
                    )));
                }
            }
        }
        results.sort_by_key(|part| part.0);

        let completed_parts: Vec<CompletedPart> = results
            .into_iter()
            .map(|(part_number, etag)| {
                CompletedPart::builder()
                    .set_part_number(Some(part_number))
                    .set_e_tag(Some(etag))
                    .build()
            })
            .collect();

        let completed_upload = CompletedMultipartUpload::builder()
            .set_parts(Some(completed_parts))
            .build();

        let response = client
            .complete_multipart_upload()
            .bucket(bucket)
            .key(key)
            .upload_id(upload_id)
            .multipart_upload(completed_upload)
            .send()
            .await
            .map_err(|e| GitError::AWSError(e.to_string()))?;

        println!("Uploaded to {:?}", response.location().unwrap_or_default());

        Ok(())
    }
}
