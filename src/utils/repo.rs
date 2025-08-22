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
use regex::Regex;

/// Hold basic information about a github url
#[derive(Debug)]
pub struct RepoInfo {
    /// Name of the repository
    pub repo_name: String,
    /// The owner of this repository
    pub owner: String,
    /// The full URL of the repository
    pub url: String,
    /// The main branch of the repository, if known
    pub main_branch: Option<String>,
}

/// A holder for things that can be checked for a repository
#[derive(Debug)]
pub struct RepoChecks {
    /// Check if the main branch is protected
    pub protected: bool,
    /// Check the license of the repository
    pub license: bool,
    /// Check for old branches.
    /// The tuple is whether to enable it, and the minimum number of days of inactivity
    pub old_branches: (bool, i64),
    /// A regex filter for the branches
    pub branch_filter: Option<Regex>,
}

pub enum CheckResult {
    /// Whether the main branch is protected
    Protected(bool),
    /// The license of the repository
    License(String),
    /// Old branches
    OldBranches(Vec<(String, String)>),
}

/// Parse the owner and repository name from a github repository url.
pub fn get_repo_info_from_url(url: &str) -> Result<RepoInfo, GitError> {
    // Named capture groups for the owner and the repo
    let repo_re =
        Regex::new(r"^https://github.com/(?<owner>[^/].+)/(?<repo>[^/].+[^/])/?(\.git)?/?.*");
    let repo_regex = match repo_re {
        Ok(re) => re,
        Err(e) => return Err(GitError::RegexError(e)),
    };
    if let Some(captures) = repo_regex.captures(url) {
        if captures.len() > 2 {
            let owner = captures["owner"].to_string();
            let repo = captures["repo"].to_string().replace(".git", "");
            return Ok(RepoInfo {
                owner,
                repo_name: repo,
                url: url.to_string(),
                main_branch: None,
            });
        }
    }
    Err(GitError::InvalidRepository(url.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_repo_info_from_url() {
        let test_cases = vec![
            (
                "https://github.com/acceldata-io/kudu",
                "acceldata-io",
                "kudu",
            ),
            (
                "https://github.com/acceldata-io/kudu/",
                "acceldata-io",
                "kudu",
            ),
            (
                "https://github.com/acceldata-io/hadoop",
                "acceldata-io",
                "hadoop",
            ),
            (
                "https://github.com/acceldata-io/trino",
                "acceldata-io",
                "trino",
            ),
            (
                "https://github.com/acceldata-io/airflow.git",
                "acceldata-io",
                "airflow",
            ),
        ];
        for u in test_cases {
            let info = get_repo_info_from_url(u.0).unwrap();
            assert_eq!(u.1, info.owner);
            assert_eq!(u.2, info.repo_name);
        }
    }
    #[test]
    fn test_get_repo_info_from_bad_url() {
        let bad_tests = vec![
            "https://github.com/",
            "github.com",
            "abcdefg",
            "https://github.com/acceldata-io/",
        ];
        for test in bad_tests {
            let info = get_repo_info_from_url(test);
            assert!(info.is_err())
        }
    }
}
