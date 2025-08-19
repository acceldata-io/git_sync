use regex::Regex;
use crate::error::GitError;

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
    pub license: bool
}

pub enum CheckResult {
    /// Whether the main branch is protected
    Protected(bool),
    /// The license of the repository
    License(String),
}

/// Parse the owner and repository name from a github repository url.
pub fn get_repo_info_from_url(url: &str) -> Result<RepoInfo, GitError> {
    // Named capture groups for the owner and the repo
    let repo_re = Regex::new(r"^https://github.com/(?<owner>[^/].+)/(?<repo>[^/].+)(\.git)?/?.*");
    let repo_regex = match repo_re {
        Ok(re) => re,
        Err(e) => return Err(GitError::RegexError(e)),
    };
    if let Some(captures) = repo_regex.captures(url) {
        if captures.len() > 2 {
            let owner = captures["owner"].to_string();
            let repo = captures["repo"].to_string().replace(".git", "");
            println!("Owner: {owner}, repo: {repo}");
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
/*
/// Handle responses to api calls
pub fn handle_response<T, F, E>(
    response: Result<T, E>,
    context: &str,
    ok: F,
) -> Result<(), GitError>
where
    F: FnOnce(T) -> Result<(), GitError>,
    E: std::fmt::Display,
{
    match response {
        Ok(data) => ok(data),
        Err(e) => Err(GitError::ApiError(format!(
            "Failed to {}: {}",
            context, e
        ))),
    }
}
*/

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_repo_info_from_url() {
        let test_cases = vec![
            ("https://github.com/acceldata-io/kudu", "acceldata-io", "kudu"),
            ("https://github.com/acceldata-io/hadoop","acceldata-io", "hadoop"),
            ("https://github.com/acceldata-io/trino","acceldata-io", "trino"),
            ("https://github.com/acceldata-io/airflow.git","acceldata-io", "airflow"),
        ];
        for u in test_cases{
            let info = get_repo_info_from_url(u.0).unwrap();
            assert_eq!(u.1, info.owner);
            assert_eq!(u.2, info.repo_name);
        }
    }
}

