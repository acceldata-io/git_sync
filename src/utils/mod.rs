use regex::Regex;
use crate::error::GitError;
use std::process::Command;

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

/// Hold information about the git user
#[derive(Debug)]
pub struct UserDetails {
    /// Name of the user
    pub name: String,
    /// Email of the user
    pub email: String,
}
/// Parse the owner and repository name from a github repository url.
pub fn get_repo_info_from_url(url: &str) -> Result<RepoInfo, GitError> {
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

impl UserDetails {
    /// Create a new UserDetails instance
    pub fn new(name: String, email: String) -> Self {
        UserDetails { name, email }
    }
    /// Try to load user information from the environment through git
    pub fn new_from_git() -> Result<Self, std::io::Error> {
        let name_output = Command::new("git").arg("config").arg("get").arg("user.name").output()?;
        let email_output = Command::new("git").arg("config").arg("get").arg("user.email").output()?;

        match (name_output.status.success(), email_output.status.success()) {
            (true, true) => {
                let name = String::from_utf8_lossy(&name_output.stdout).trim().to_string();
                let email = String::from_utf8_lossy(&email_output.stdout).trim().to_string();
                Ok(UserDetails { name, email })
            }
            (false, _) => Err(std::io::Error::other("Failed to get git user name")),
            (_, false) => Err(std::io::Error::other("Failed to get git email address")),
        }
    }
}

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
