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
use crate::utils::compress::compress_directory;
use crate::utils::repo::http_to_ssh_repo;
use futures::stream::{FuturesUnordered, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashSet;
use std::fmt::Display;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::{path::Path, process::Stdio};
use tokio::sync::OnceCell;

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

#[cfg(feature = "aws")]
/// The size of each part of a multipart file upload to s3.
const PART_SIZE: usize = 64 * 1024 * 1024;

impl GithubClient {
    /// Backup a single repository into the specified folder. If the backup already exists, try to
    /// update it instead of cloning again.
    pub async fn backup_repo<T: AsRef<str>>(
        &self,
        url: T,
        path: &Path,
    ) -> Result<PathBuf, GitError> {
        let semaphore_lock = Arc::clone(&self.semaphore).acquire_owned().await?;
        let canonical_path = if let Ok(p) = path.canonicalize() {
            p
        } else {
            path.to_path_buf()
        };

        let ssh_url = http_to_ssh_repo(url.as_ref())?;

        let Some(name) = ssh_url.split('/').next_back() else {
            return Err(GitError::Other(format!(
                "Failed to extract repository name from URL: {ssh_url}"
            )));
        };

        let mut path = canonical_path.to_string_lossy().to_string();
        path.push('/');
        path.push_str(name);
        let output_path = path.clone();

        if let Some(t) = self.output.get()
            && *t == OutputMode::Print
        {
            println!("Backing up repository {} to path: {path}", url.as_ref());
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
                let name = ssh_url
                    .clone()
                    .split('/')
                    .next_back()
                    .unwrap_or("")
                    .to_string();
                match output {
                    // This branch happens only if the output of the command was successful
                    Ok(s) if s.status.success() => Ok::<_, GitError>(format!(
                        "Successfully backed up repository: {name} to path: {path}"
                    )),
                    Ok(s) => {
                        let stderr = String::from_utf8_lossy(&s.stderr);
                        eprintln!("Error while backing up.");
                        eprintln!("STDERR:\n{stderr}");
                        Err(GitError::Other(format!(
                            "Failed to back up repository: {name}, git exited with status: {}",
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
                // Clone the repository as a mirror
                let output = Command::new("git")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .args(["clone", "--mirror", &ssh_url, &path])
                    .output();
                let name = ssh_url
                    .clone()
                    .split('/')
                    .next_back()
                    .unwrap_or("")
                    .to_string();

                match output {
                    // This branch happens only if the output of the command was successful
                    Ok(s) if s.status.success() => Ok::<_, GitError>(format!(
                        "\tSuccessfully backed up repository: {name} to path: {path}"
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
                            "Failed to back up repository: {name}, git exited with status: {}",
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

        // Match outside the spawn_blocking to avoid ownership issues
        match result {
            Ok(m) => {
                if let Some(t) = self.output.get()
                    && *t == OutputMode::Print
                    && self.is_tty
                {
                    println!("{m}");
                }
                self.append_slack_message(m).await;
            }
            Err(e) => {
                if let Some(t) = self.output.get()
                    && *t == OutputMode::Print
                    && self.is_tty
                {
                    eprintln!("{e}");
                }
                self.append_slack_error(e.to_string()).await;
                return Err(e);
            }
        }

        Ok(output_path.into())
    }
    /// Backup all configured repositories into the specified folder. This can take a long time,
    /// depending on the number of repositories and their sizes
    pub async fn backup_all_repos(
        &self,
        repositories: &[impl ToString],
        path: &Path,
        blacklist: HashSet<String>,
    ) -> Result<Vec<PathBuf>, GitError> {
        let mut futures = FuturesUnordered::new();
        let filtered_repositories: Vec<String> = repositories
            .iter()
            .filter(|r| !blacklist.contains(&r.to_string()))
            .map(std::string::ToString::to_string)
            .collect();

        let number_of_repos = filtered_repositories.len();

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
            progress.println("Backing up all selected repositories...");
        }

        for repo in filtered_repositories {
            futures.push(async move {
                let result = self.backup_repo(&repo, path).await;
                (result, repo)
            });
        }
        let mut successful_backup_paths: Vec<PathBuf> = Vec::new();
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((result, repo)) = futures.next().await {
            if let Some(ref progress) = progress {
                progress.inc(1);
            }
            match result {
                Ok(path) => {
                    self.append_slack_message(format!(
                        "Backed up repository: {repo} to {}",
                        path.display()
                    ))
                    .await;

                    successful_backup_paths.push(path);

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
                    errors.push((repo.to_string(), e));
                }
            }
        }
        if let Some(progress) = progress {
            progress.finish_with_message("Finished cloning backups");
        }
        Ok(successful_backup_paths)
    }
    /// This is a way to clean up stale references in a configured backed up repository. This is
    /// still under development, so don't rely on it too much yet.
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
        let _lock = self.semaphore.clone().acquire_owned().await?;
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
                if let Some(t) = self.output.get()
                    && *t == OutputMode::Print
                    && self.is_tty
                {
                    println!("{m}");
                }
                self.append_slack_message(m).await;
            }
            Err(e) => {
                if let Some(t) = self.output.get()
                    && *t == OutputMode::Print
                    && self.is_tty
                {
                    eprintln!("{e}");
                }
                self.append_slack_error(e.to_string()).await;
                return Err(e);
            }
        }

        Ok(())
    }
    #[cfg(feature = "aws")]
    /// Upload a backup to s3. This requires your aws credentials to be configured in the
    /// environment. For small enough files, it will do a single part file upload but if the file
    /// is bigger than `PART_SIZE`, it will instead do a parallel multipart upload.
    ///
    /// The path needs to point to an uncompressed git mirror. It will be compressed before upload,
    /// and the compressed file will be removed after upload.
    #[allow(clippy::too_many_lines)]
    pub async fn backup_to_s3<T: AsRef<str>>(
        &self,
        file_path: &Path,
        bucket: T,
    ) -> Result<(), GitError> {
        let region = RegionProviderChain::default_provider().or_else("ca-central-1");
        let region_name = if let Some(name) = region.region().await {
            name.to_string()
        } else {
            String::new()
        };

        // Use the behaviour version from when this was developed
        let config = aws_config::defaults(BehaviorVersion::v2025_08_07())
            .region(region)
            .load()
            .await;

        let client = Arc::new(Client::new(&config));

        if !file_path.exists() {
            return Err(GitError::FileDoesNotExist(
                file_path.to_string_lossy().to_string(),
            ));
        }

        if !file_path.is_dir() {
            return Err(GitError::Other(format!(
                "File {} is not a directory",
                file_path.display()
            )));
        }
        if let Some(ext) = file_path.extension()
            && ext != "git"
        {
            return Err(GitError::Other(format!(
                "File {} is not a git mirror",
                file_path.display()
            )));
        }
        let compressed_dir = compress_directory(file_path)?;
        // We know that this directory exists, so we can unwrap it safely
        let key_name = compressed_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();

        let mut file = File::open(&compressed_dir).await?;
        file.set_max_buf_size(PART_SIZE);
        let mut file_buffer = BufReader::new(file);
        let mut part_number = 1;

        // If the file is smaller than the minimum size allowed for multipart uploads, or it's
        // smaller than the minimum part size, just upload the whole thing
        let size = file_buffer.get_ref().metadata().await?.len();
        if size == 0 {
            return Err(GitError::Other("Backup file is empty".to_string()));
        }

        if size < PART_SIZE as u64 {
            let body = aws_sdk_s3::primitives::ByteStream::from_path(&compressed_dir)
                .await
                .map_err(|e| GitError::Other(e.to_string()))?;
            client
                .put_object()
                .bucket(bucket.as_ref())
                .key(key_name)
                .content_type("application/gzip")
                .body(body)
                .send()
                .await
                .map_err(|e| GitError::AWSError(e.to_string()))?;

            // The else branch should never happen, but just in case...
            let name: String = if let Some(file) = compressed_dir.file_name() {
                file.to_string_lossy().to_string()
            } else {
                String::new()
            };

            let s3_url = format!(
                "https://{}.s3.{region_name}.amazonaws.com/{name}",
                bucket.as_ref()
            );

            let string_name = name.to_string();
            let base_name = string_name.split('/').next_back().unwrap_or("");
            let message = format!("\t• Uploaded backup {base_name} to {s3_url}");

            self.append_slack_message(&message).await;

            if self.is_tty {
                println!("{message}",);
            }
            let removal = std::fs::remove_file(&compressed_dir);
            if removal.is_err() {
                eprintln!(
                    "Failed to remove tarball backup file {}",
                    compressed_dir.display(),
                );
            }

            return Ok(());
        }

        // Initiate the multipart upload
        let response = client
            .create_multipart_upload()
            .bucket(bucket.as_ref())
            .key(&key_name)
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
            let bucket = bucket.as_ref().to_string();
            let upload_id = upload_id.to_string();
            let local_part_number = part_number;

            // Acquire the semaphore lock here to limit the number of concurrent uploads
            let lock = Arc::clone(&self.semaphore).acquire_owned().await?;
            if self.is_tty && self.output.get() != Some(&OutputMode::Progress) {
                println!("Starting upload of part {local_part_number}");
            }
            let is_tty = self.is_tty;
            let output_type: Arc<OnceCell<OutputMode>> = Arc::clone(&self.output);
            let key = key_name.to_string();

            futures.push(tokio::spawn(async move {
                let bucket = bucket.to_string();
                let _lock = lock;
                let response = client
                    .upload_part()
                    .bucket(bucket)
                    .key(key)
                    .upload_id(&upload_id)
                    .part_number(local_part_number)
                    .body(buffer.into())
                    .send()
                    .await
                    .map_err(|e| GitError::AWSError(e.to_string()))?;
                let etag = response.e_tag().unwrap().to_string();

                if is_tty && output_type.get() != Some(&OutputMode::Progress) {
                    println!("Uploaded part {local_part_number}");
                }

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
            .bucket(bucket.as_ref())
            .key(key_name)
            .upload_id(upload_id)
            .multipart_upload(completed_upload)
            .send()
            .await
            .map_err(|e| GitError::AWSError(e.to_string()))?;

        self.append_slack_message(format!(
            "Uploaded backup {} to {}",
            file_path.display(),
            response.location().unwrap_or_default()
        ))
        .await;

        if self.is_tty {
            println!(
                "Uploaded backup {} to {}",
                compressed_dir.display(),
                response.location().unwrap_or_default()
            );
        }

        let removal = std::fs::remove_file(&compressed_dir);
        if removal.is_err() {
            eprintln!(
                "Failed to remove tarball backup file {}",
                compressed_dir.display(),
            );
        }
        Ok(())
    }
    /// Backup all repositories to S3
    pub async fn backup_all_to_s3<T: AsRef<str> + Display>(
        &self,
        paths: Vec<PathBuf>,
        bucket: T,
    ) -> Result<(), GitError> {
        let mut errors: Vec<(String, GitError)> = Vec::new();
        let mut futures = FuturesUnordered::new();

        let progress = if self.is_tty {
            self.output.set(OutputMode::Progress).ok();
            let pb = ProgressBar::new(paths.len() as u64);
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
        for path in &paths {
            let bucket = bucket.as_ref();
            let file_path = path.canonicalize()?;
            futures.push(async move {
                let result = self.backup_to_s3(&file_path, bucket).await;
                (file_path, result)
            });
        }
        while let Some((path, result)) = futures.next().await {
            match result {
                Ok(()) => {
                    if let Some(ref pb) = progress {
                        pb.inc(1);
                        pb.println(format!("Uploaded {} to {bucket}", path.to_string_lossy()));
                    }
                }
                Err(e) => {
                    errors.push((path.to_string_lossy().to_string(), e));
                }
            }
        }
        if let Some(ref pb) = progress {
            pb.finish_with_message("Finished backing up to S3");
        }
        if !errors.is_empty() {
            return Err(GitError::MultipleErrors(errors));
        }
        Ok(())
    }
}
