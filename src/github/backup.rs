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
use crate::github::client::GithubClient;
use std::path::Path;
use std::process::Command;

impl GithubClient {
    pub async fn backup_repo(&self, ssh_url: &str, path: &Path) -> Result<(), GitError> {
        let _semaphore_lock = self.semaphore.clone().acquire_owned().await?;
        let ssh_url = ssh_url.to_string();
        let canonical_path = if let Ok(p) = path.canonicalize() {
            p
        } else {
            path.to_path_buf()
        };
        let path = canonical_path.to_string_lossy().to_string();
        let result = tokio::task::spawn_blocking(move || {
            // Clone the reposittory as a mirror
            let output = Command::new("git")
                .args(["clone", "--mirror", &ssh_url, &path])
                .status();
            match output {
                Ok(s) if s.success() => Ok::<_, GitError>(format!(
                    "Successfully backed up repository: {ssh_url} to path: {path}"
                )),
                Ok(s) => Err(GitError::Other(format!(
                    "Failed to back up repository: {ssh_url}, git exited with status: {s}"
                ))),
                Err(e) => Err(GitError::Other(format!(
                    "Failed to back up repository: {ssh_url}, error: {e}"
                ))),
            }
        })
        .await?;

        match result {
            Ok(m) => {
                if self.is_tty {
                    println!("{m}");
                }
                self.append_slack_message(m).await;
            }
            Err(e) => {
                if self.is_tty {
                    eprintln!("{e}");
                }
                self.append_slack_error(e.to_string()).await;
                return Err(e);
            }
        }

        Ok(())
    }
    /*pub async fn backup_to_s3(&self, ssh_url: &str, file_path: &Path, bucket: &str, key: &str) -> Result<(), GitError> {
        Ok(())
    }*/
}
