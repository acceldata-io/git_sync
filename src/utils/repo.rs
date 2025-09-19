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
use fancy_regex::Regex;
use serde::Deserialize;
use std::fmt;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

// Initialized once, then it becomes available
// from then on so we don't have to compile our regex every
// single time test_get_repo_info_from_url is called
static REPO_REGEX: OnceLock<Regex> = OnceLock::new();

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

/// Struct for holding tag information.
/// This is for both annotated and lightweight tags.
#[derive(Debug, Deserialize, Clone)]
pub struct TagInfo {
    /// Name of the tag
    pub name: String,
    /// The type of tag (Annotated or lightweight)
    pub tag_type: TagType,
    /// SHA of the tag
    pub sha: String,
    /// Git URL from where the tag was fetched
    pub url: String,
    /// SHA of annotated tag commit
    pub commit_sha: Option<String>,
}

/// Implements checking for equality based on the tag name only. This is needed for
/// `Taginfo` to be used in a `HashSet`
impl PartialEq for TagInfo {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}
impl Eq for TagInfo {}

/// Implements hashing for name only. This is needed to use `TagInfo` in a `HashSet`
impl Hash for TagInfo {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

/// The different types of tags
#[derive(Debug, Deserialize, PartialEq, Eq, Hash, Clone)]
pub enum TagType {
    Annotated,
    Lightweight,
}

/// A holder for the results of various checks on a repository
#[derive(Debug, Clone)]
pub struct Checks {
    pub branches: Vec<(String, String)>,
    /// Certain types of branch protection rules can be queried. This holds those if they are
    /// available
    pub rules: Vec<BranchProtectionRule>,
    /// The license of the repository, if known
    pub license: Option<LicenseInfo>,
    /// Then name of the repository we're checking
    pub repo: String,
}

/// Struct to hold branch protection rule information. Exists in order to deserialize JSON into
/// this struct
#[derive(Debug, Deserialize, Clone)]
pub struct BranchProtectionRule {
    pub pattern: Option<String>,
    #[serde(rename = "isAdminEnforced")]
    pub admin_enforced: Option<bool>,
    #[serde(rename = "requiresApprovingReviews")]
    pub requires_approving_reviews: Option<bool>,
    #[serde(rename = "requiredApprovingReviewCount")]
    pub requires_approving_review_count: Option<i64>,
    #[serde(rename = "requiresStatusChecks")]
    pub requires_status_checks: Option<bool>,
    #[serde(rename = "requiresStrictStatusChecks")]
    pub requires_strict_status_checks: Option<bool>,
    #[serde(rename = "restrictPushes")]
    pub restricts_pushes: Option<bool>,
    #[serde(rename = "restrictsReviewDismissals")]
    pub restricts_review_dismissals: Option<bool>,
}
fn opt_bool(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "Yes",
        Some(false) => "No",
        None => "N/A",
    }
}

/// Implement display for  `BranchProtectionRule` so that it can be printed nicely
impl fmt::Display for BranchProtectionRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut output = String::new();

        let pattern = format!("\tPattern: {}\n", self.pattern.as_deref().unwrap_or("N/A"));
        let admin = format!("\tAdmin enforced: {}\n", opt_bool(self.admin_enforced));

        let pr = opt_bool(self.requires_approving_reviews);
        let require_pr = format!("\tRequire PR approving reviews: {pr}\n");
        let status_check = format!(
            "\tRequire status checks: {}\n",
            opt_bool(self.requires_status_checks)
        );
        let strict_check = format!(
            "\tRequire strict status checks: {}\n",
            opt_bool(self.requires_strict_status_checks)
        );
        let restrict_pushes = format!("\tRestrict pushes: {}\n", opt_bool(self.restricts_pushes));
        let restrict_dismissals = format!(
            "\tRestrict review dismissals: {}\n\n",
            opt_bool(self.restricts_review_dismissals)
        );

        write!(output, "{pattern}")?;
        write!(output, "{admin}")?;

        if let Some(count) = self.requires_approving_review_count {
            let out = format!("\tRequired PR review count: {count}\n");
            write!(output, "{out}")?;
        } else {
            writeln!(output, "\tRequired PR review count: None")?;
        }

        write!(output, "{status_check}")?;
        write!(output, "{require_pr}")?;
        write!(output, "{strict_check}")?;
        write!(output, "{restrict_pushes}")?;
        write!(output, "{restrict_dismissals}")?;

        write!(f, "{output}")
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct LicenseInfo {
    pub name: Option<String>,
    #[serde(rename = "spdxId")]
    pub spdx_id: Option<String>,
}

/// A holder for things that can be checked for a repository
#[derive(Debug, Clone)]
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

/// Parse the owner and repository name from a github repository url.
pub fn get_repo_info_from_url(url: &str) -> Result<RepoInfo, GitError> {
    // Named capture groups for the owner and the repo
    // Use the OnceLock to ensure we only compile the regex once, improving performance greatly
    // compared to compiling the regex each time
    let repo_regex = REPO_REGEX.get_or_init(|| {
        // This has an optional prefix for github, and a required owner and repo.
        // We don't need the .git suffix at the end, so while we do optionally check for it,
        // we don't need to capture it.
        //
        // "The repository name can only contain ASCII letters, digits, and the characters ., -, and _."
        let msg = format!("Invalid regex for {url}");
        Regex::new(
            r"^(?:https://github.com/)?(?P<owner>[A-Za-z0-9._-]+)/(?P<repo>[A-Za-z0-9._-]+)(?:\.git)?/?$",
        )
        .expect(&msg)
    });
    let url = url.trim();
    if let Some(captures) = repo_regex.captures(url)? {
        let owner = match captures.name("owner") {
            Some(m) => m.as_str().to_string(),
            None => return Err(GitError::InvalidRepository(url.to_string())),
        };

        let repo = match captures.name("repo") {
            Some(m) => m
                .as_str()
                .strip_suffix(".git")
                .unwrap_or(m.as_str())
                .to_string(),
            None => return Err(GitError::InvalidRepository(url.to_string())),
        };
        let url = format!("https://github.com/{owner}/{repo}");
        return Ok(RepoInfo {
            owner,
            url,
            repo_name: repo,
            main_branch: None,
        });
    }
    Err(GitError::InvalidRepository(url.to_string()))
}

/// Convert an https github url to a clonable SSH URL, needed for cloning a repository
/// in order to be able to push to the repository
pub fn http_to_ssh_repo(url: &str) -> Result<String, GitError> {
    let url = url.trim();
    if url.starts_with("git@github.com") {
        #[allow(clippy::case_sensitive_file_extension_comparisons)]
        if url.ends_with(".git") {
            return Ok(url.to_string());
        }
        return Ok(format!("{url}.git"));
    }
    let repo_info = get_repo_info_from_url(url)?;
    Ok(format!(
        "git@github.com:{}/{}.git",
        repo_info.owner, repo_info.repo_name
    ))
}

/// Convert github http status errors to a usable string message
pub fn get_http_status(err: &octocrab::Error) -> (Option<http::StatusCode>, Option<String>) {
    if let octocrab::Error::GitHub { source, .. } = err {
        let status = source.status_code;
        let message = source.message.clone();
        return (Some(status), Some(message));
    }
    (None, None)
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
            assert!(info.is_err());
        }
    }
}
