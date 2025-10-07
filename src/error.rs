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
use console::style;
use std::error::Error;
use std::fmt::Write as _;
use std::io::ErrorKind::{
    AddrInUse, AddrNotAvailable, BrokenPipe, ConnectionAborted, ConnectionRefused, ConnectionReset,
    HostUnreachable, NetworkUnreachable, NotConnected, TimedOut, WouldBlock,
};
use thiserror::Error;

/// Error type to make it easier to handle returning from functions
#[derive(Error, Debug)]
pub enum GitError {
    /// Generic holder for multiple errors
    #[error("Multiple errors: {0:?}")]
    MultipleErrors(Vec<(String, GitError)>),
    /// Convert octocrab errors into our own error type
    #[error("Github API error: {0}")]
    GithubApiError(#[from] octocrab::Error),
    /// PR Merge error
    #[error("PR #{0} not mergeable")]
    PRNotMergeable(
        /// Pull Request number
        u64,
    ),
    /// No upstream repository found for a fork
    #[error("Upstream not found for {0}")]
    NoUpstreamRepo(
        /// Repository
        String,
    ),

    #[cfg(feature = "aws")]
    #[error("AWS error: {0}")]
    /// Generic AWS error
    AWSError(String),
    /// Failure to parse date
    #[error("Date parse error: {0}")]
    DateParseError(#[from] chrono::ParseError),
    /// URL is not a vaild repository
    #[error("Invalid repository URL: {0}")]
    InvalidRepository(String),
    /// Generic sync failure
    #[error("Could not sync {ref_type} for {repository}")]
    SyncFailure {
        ref_type: String,
        repository: String,
    },
    /// This error is too big, so we wrap it in a Box, then implement the conversion below
    #[error("Regex error: {0}")]
    RegexError(#[from] Box<fancy_regex::Error>),
    /// Repository list is empty
    #[error("No repos configured")]
    NoReposConfigured,
    /// Repository is not a fork for an action that requires a fork
    #[error("Repository is not a fork")]
    NotAFork,
    /// No repository name was provided
    #[error("Missing repository name")]
    MissingRepositoryName,
    /// Some git reference cannot be found
    #[error("No such reference: {0}")]
    NoSuchReference(
        /// Git Reference
        String,
    ),
    /// Branch does not exist
    #[error("No such branch: {0}")]
    NoSuchBranch(
        /// Branch name
        String,
    ),
    /// Tag does not exist
    #[error("No such tag: {0}")]
    NoSuchTag(
        /// Tag name
        String,
    ),
    /// Pull Request cannot be found
    #[error("No such pull request for {repository} with head '{head}' and base '{base}'")]
    NoSuchPR {
        /// Repository
        repository: String,
        /// head branch
        head: String,
        /// base branch
        base: String,
    },
    /// Some toml parsing error
    #[error("TOML parsing error: {0}")]
    TomlError(#[from] toml::de::Error),

    /// File could not be found
    #[error("File does not exist: {0}")]
    FileDoesNotExist(
        /// File Path
        String,
    ),
    /// Github token is missing
    #[error(
        "Missing GitHub token. Please provide a token via --token, GITHUB_TOKEN environment variable, or in your config file."
    )]
    MissingToken,
    /// Generic type for join errors from tokio tasks
    #[error("Join error: {0}")]
    JoinError(#[from] tokio::task::JoinError),
    /// Some IO error
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    /// Failure to parse JSON
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    /// Timezone could not be found
    #[error("Timezone error: {0}")]
    GetTimezoneError(#[from] iana_time_zone::GetTimezoneError),
    /// A failure to acquire a semaphore permit. If this happens, something is very wrong
    #[error("Failed to acquire semaphore: {0}")]
    SemaphoreError(#[from] tokio::sync::AcquireError),
    /// A failure to push changes to a repository
    #[error("Failed to push changes to {0}")]
    GitPushError(String),
    /// Git clone failure
    #[error("Failed to clone {repository}:{branch}")]
    GitCloneError { repository: String, branch: String },
    /// Likely a merge conflict
    #[error("Could not fast forward merge branch {branch} for repository {repository}")]
    GitFFMergeError { branch: String, repository: String },
    /// Could not upload a file to aws
    #[error("Failed to upload file: {0}")]
    FileUploadError(
        /// File Path
        String,
    ),
    /// Provided path is something other than a directory
    #[error("'{0}' is not a directory")]
    NotADirectory(
        /// File Path
        String,
    ),
    /// Did not detect a valid git mirror
    #[error("'{0}' is not a git mirror")]
    NotGitMirror(
        /// File Path
        String,
    ),
    /// Failed to run an external command
    #[error("Execution error while running {command}: {status}")]
    ExecutionError { command: String, status: String },
    /// File has no contents
    #[error("'{0}' is an empty file")]
    EmptyFile(
        /// File Path
        String,
    ),
    /// File already exists
    #[error("File {0} already exists")]
    FileExists(
        /// File Path
        String,
    ),
    /// Invalid version for ODP-bigtop
    #[error("Invalid odp-bigtop version: {0}")]
    InvalidBigtopVersion(
        /// Version String
        String,
    ),
    /// Group not found in configuration file
    #[error("Invalid group '{0}'")]
    InvalidGroup(
        /// Group Name
        String,
    ),
    /// Group exists but has no repositories in it
    #[error("Group '{0}' is empty")]
    EmptyGroup(
        /// Group Name
        /// /// Group Name
        String,
    ),
    /// Unclassified error
    #[error("{0}")]
    Other(
        /// Some message
        String,
    ),
}

impl GitError {
    /// Convert errors into a user-friendly format. Include a suggestion on how to proceed, if possible.
    #[allow(clippy::too_many_lines)]
    pub fn to_user_error(&self) -> UserError {
        match self {
            GitError::GithubApiError(e) => {
                let (code, msg) = octocrab_error_info(e);
                let category = if is_network_error(e) {
                    ErrorCategory::Network
                } else{
                    match code {
                        Some(http::StatusCode::UNAUTHORIZED | http::StatusCode::FORBIDDEN) => ErrorCategory::Auth,
                        Some(http::StatusCode::NOT_FOUND) => ErrorCategory::NotFound,
                        Some(http::StatusCode::TOO_MANY_REQUESTS) => ErrorCategory::RateLimit,
                        Some(ref c) if c.is_server_error() => ErrorCategory::Server,
                        Some(ref c) if c.is_client_error() => ErrorCategory::Input,
                        _ => ErrorCategory::Unknown,
                    }
                };
                let suggestion = match category {
                    ErrorCategory::Auth => Some("Check your credentials and repository permissions.".into()),
                    ErrorCategory::RateLimit => Some("You are being rate-limited by GitHub. Wait and try again later.".into()),
                    ErrorCategory::Network => Some("Check your Internet connection.".into()),
                    ErrorCategory::NotFound => Some("Check the repository or resource name.".into()),
                    _ => None,
                };
                UserError { code, message: msg, suggestion }
            }
            GitError::SyncFailure{ref_type, repository} => UserError {
                code: None,
                message: format!("Could not sync {ref_type} for repository {repository}"),
                suggestion: Some("Check that the your references exist in {repository}.".into()),
            },
            GitError::PRNotMergeable(pr_number) => UserError {
                code: None,
                message: format!("PR #{pr_number} cannot be merged automatically"),
                suggestion: Some("Human intervention may be required.".into()),
            },
            GitError::NoSuchPR{repository, head, base} => UserError {
                code: None,
                message: format!("No such PR for {repository} with head '{head}' and base '{base}'"),
                suggestion: Some("Check that the branch names are correct.".into()),
            },
            GitError::NoUpstreamRepo(repo) => UserError {
                code: None,
                message: format!("Could not find an upstream repository for {repo}"),
                suggestion: Some(format!("Try adding {repo} to the fork_with_workaround group in your config file")),
            },
            GitError::DateParseError(e) => UserError {
                code: None,
                message: format!("Invalid date: {e}"),
                suggestion: Some("Use a valid date format.".into()),
            },
            GitError::GitFFMergeError{branch, repository} => UserError {
                code: None,
                message: format!("Could not fast forward merge branch {branch} for repository {repository}"),
                suggestion: Some("Check that the branch exists and that there are no merge conflicts.".into()),
            },
            GitError::InvalidRepository(url) => UserError {
                code: None,
                message: format!("Invalid repository URL: {url}"),
                suggestion: Some("Check the repository URL.".into()),
            },
            GitError::InvalidBigtopVersion(version) => UserError {
                code: None,
                message: format!("Invalid odp-bigtop version: {version}"),
                suggestion: Some("Try fixing your version string; if you do not want to modify the version of files and build numbers in odp-bigtop, add the --not-version flag".into()),
            },
            GitError::RegexError(e) => UserError {
                code: None,
                message: format!("Regex error: {e}"),
                suggestion: Some("Check your regular expression syntax.".into()),
            },
            GitError::NoReposConfigured => UserError {
                code: None,
                message: "No repositories are configured.".into(),
                suggestion: Some("Add repositories to your configuration.".into()),
            },
            GitError::NotAFork => UserError {
                code: None,
                message: "Repository is not a fork.".into(),
                suggestion: Some("Choose a repository that is a fork.".into()),
            },
            GitError::MissingRepositoryName => UserError {
                code: None,
                message: "Repository name is missing.".into(),
                suggestion: Some("Specify the repository name.".into()),
            },
            GitError::InvalidGroup(group) => UserError {
                code: None,
                message: format!("Invalid group '{group}'"),
                suggestion: Some("Check your config file for typos.".into()),
            },
            GitError::EmptyGroup(group) => UserError {
                code: None,
                message: format!("Group '{group}' is empty"),
                suggestion: Some("Add repositories to '{group}' in your config file.".into()),
            },
            GitError::GitCloneError{repository, branch} => UserError {
                code: None,
                message: format!("Failed to clone {repository}:{branch}"),
                suggestion: Some(format!("Check that the {repository} and the branch '{branch}' exist and that you have access.")),
            },
            GitError::GitPushError(repository) => UserError {
                code: None,
                message: format!("Failed to push changes to {repository}"),
                suggestion: Some(format!("Check that you have write access to '{repository}' and that your local branch is up to date.")),
            },
            GitError::NoSuchBranch(branch) => UserError {
                code: None,
                message: format!("No such branch: {branch}"),
                suggestion: Some("Check the branch name or create it first.".into()),
            },
            GitError::NoSuchTag(tag) => UserError {
                code: None,
                message: format!("No such tag: {tag}"),
                suggestion: Some("Check the tag name or create it first.".into()),
            },
            GitError::NoSuchReference(reference) => UserError {
                code: None,
                message: format!("No such reference: {reference}"),
                suggestion: Some(format!("Make sure your reference {reference} exists")),
            },
            GitError::ExecutionError{command, status} => UserError {
                code: None,
                message: format!("Execution error while running '{command}': '{status}'"),
                suggestion: Some(format!("Ensure that '{command}' is correct and that the base command is available in your PATH")),
            },
            GitError::FileDoesNotExist(file) => UserError {
                code: None,
                message: format!("'{file}' does not exist"),
                suggestion: Some("Check that you passed the right path.".into()),
            },
            GitError::EmptyFile(file) => UserError {
                code: None,
                message: format!("'{file}' is an empty file"),
                suggestion: Some("Try restarting the backup process.".into()),
            },
            GitError::NotADirectory(dir) => UserError {
                code: None,
                message: format!("'{dir}' is not a directory"),
                suggestion: Some("Check that you passed the right path.".into()),
            },
            GitError::NotGitMirror(dir) => UserError {
                code: None,
                message: format!("'{dir}' is not a git mirror"),
                suggestion: Some("Check that you passed the right path.".into()),
            },
            GitError::FileExists(file) => UserError {
                code: None,
                message: format!("File '{file}' already exists"),
                suggestion: Some("Use --force to overwrite the existing file.".into()),
            },
            GitError::FileUploadError(msg) => UserError {
                code: None,
                message: format!("Failed to upload file: {msg}"),
                suggestion: Some("Check the file path and permissions.".into()),
            },
            GitError::TomlError(e) => UserError {
                code: None,
                message: format!("TOML parsing error: {e}"),
                suggestion: Some("Check your config file for syntax errors.".into()),
            },
            GitError::MissingToken => UserError {
                code: None,
                message: "Missing GitHub token. Please provide a token via --token, GITHUB_TOKEN environment variable, or in your config file.".into(),
                suggestion: Some("Provide a GitHub token using --token, environment variable, or config file.".into()),
            },
            GitError::JoinError(e) => UserError {
                code: None,
                message: format!("Failed to join tasks: {e}"),
                suggestion: None,
            },
            #[cfg(feature = "aws")]
            GitError::AWSError(e) => UserError {
                code: None,
                message: format!("AWS Error: {e}"),
                suggestion: None,
            },
            GitError::IoError(e) => UserError {
                code: None,
                message: format!("IO error: {e}"),
                suggestion: Some("Check file paths and permissions, or try again.".into()),
            },
            GitError::JsonError(e) => UserError {
                code: None,
                message: format!("JSON error: {e}"),
                suggestion: Some("Check your JSON syntax.".into()),
            },
            GitError::GetTimezoneError(e) => UserError {
                code: None,
                message: format!("Timezone error: {e}"),
                suggestion: Some("Check your timezone string or system settings.".into()),
            },
            GitError::SemaphoreError(e) => UserError {
                code: None,
                message: format!("Semaphore Error: {e}"),
                suggestion: None,
            },

            GitError::Other(msg) => UserError {
                code: None,
                message: msg.clone(),
                suggestion: None,
            },
            GitError::MultipleErrors(errors) => {
                // Show the first error, but mention there were multiple
                if errors.is_empty() {
                    UserError {
                        code: None,
                        message: "Multiple unknown errors occurred.".into(),
                        suggestion: None,
                    }
                } else  {
                    let mut message = "Multiple errors: ".to_string();
                    for (context, err) in errors {
                        let e = err.to_user_error();
                        let _ = writeln!(message, "\n\t- {context}: {}", e.message);
                    }
                    UserError {
                        code: None,
                        message,
                        suggestion: None,
                    }
                }
            }
        }
    }
    /// Walk through the error chain to find out if it's a broken pipe
    pub fn is_broken_pipe(&self) -> bool {
        let mut source = Some(self as &dyn std::error::Error);
        while let Some(err) = source {
            if let Some(ioe) = err.downcast_ref::<std::io::Error>()
                && ioe.kind() == std::io::ErrorKind::BrokenPipe
            {
                return true;
            }
            source = err.source();
        }
        false
    }
}

impl From<fancy_regex::Error> for GitError {
    fn from(err: fancy_regex::Error) -> Self {
        GitError::RegexError(Box::new(err))
    }
}
/// Different categories of errors for user-friendly messages
#[derive(Debug)]
pub enum ErrorCategory {
    NotFound,
    Auth,
    RateLimit,
    Network,
    Input,
    Server,
    Unknown,
}

/// Holds information about an error to present to the user
#[derive(Debug)]
pub struct UserError {
    pub code: Option<http::StatusCode>,
    pub message: String,
    pub suggestion: Option<String>,
}

impl std::fmt::Display for UserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(code) = self.code {
            let message = format!("HTTP error ({code:?}): {}", self.message);
            let formatted = format!("{}", style(message).red().bold());
            writeln!(f, "{formatted}")?;
        } else {
            let message = format!("Error: {}", self.message);
            writeln!(f, "{}", style(message).red().bold())?;
        }

        if let Some(suggestion) = &self.suggestion {
            let message = format!("Suggestion: {suggestion}");
            write!(f, "{}", style(message).green())?;
        }
        Ok(())
    }
}

/// Check if whatever error is passed is reasonably retryable. Most errors aren't, but there are a
/// few we can try to repeat.
pub fn is_retryable(e: &octocrab::Error) -> bool {
    match e {
        octocrab::Error::GitHub { source, .. } => {
            // If either of these are true, then we
            let msg = &source.message;
            let status = source.status_code;

            if status.is_server_error() {
                return true;
            } else if status == http::StatusCode::FORBIDDEN {
                // Sometimes a 403 is returned for rate limiting
                if msg.contains("rate limit") || msg.contains("abuse detection") {
                    eprintln!("Rate limited by GitHub API: {msg}");
                }
                return false;
            } else if status == http::StatusCode::TOO_MANY_REQUESTS {
                eprintln!("Rate limited by GitHub API: {msg}");
                return false;
            } else if status.is_client_error() {
                return false;
            }
            false
        }
        octocrab::Error::Http { .. }
        | octocrab::Error::Hyper { .. }
        | octocrab::Error::Service { .. } => true,
        _ => false,
    }
}

/// Helper function to extract errors from an `octocrab::Error`
pub fn octocrab_error_info(e: &octocrab::Error) -> (Option<http::StatusCode>, String) {
    match e {
        octocrab::Error::GitHub { source, .. } => {
            let status = source.status_code;
            let mut message = source.message.to_string();
            if let Some(errors) = &source.errors {
                for err in errors {
                    if let Some(obj) = err.as_object()
                        && let Some(msg) = obj.get("message").and_then(|m| m.as_str())
                    {
                        let m = format!("\n- {msg}");
                        let _ = write!(message, "{m}");
                    }
                }
            }
            (Some(status), message.to_string())
        }
        octocrab::Error::Http { source, .. } => (None, source.to_string()),
        octocrab::Error::UriParse { source, .. } => (None, source.to_string()),
        octocrab::Error::Uri { source, .. } => (None, source.to_string()),
        octocrab::Error::Installation { .. } => (None, "Installation error".to_string()),
        octocrab::Error::InvalidHeaderValue { source, .. } => (None, source.to_string()),
        octocrab::Error::InvalidUtf8 { source, .. } => (None, source.to_string()),
        octocrab::Error::Encoder { source, .. } => (None, source.to_string()),
        octocrab::Error::Hyper { source, .. } => (None, source.to_string()),
        octocrab::Error::SerdeUrlEncoded { source, .. } => (None, source.to_string()),
        octocrab::Error::Serde { source, .. } => (None, source.to_string()),
        octocrab::Error::Json { source, .. } => (None, source.to_string()),
        octocrab::Error::JWT { source, .. } => (None, source.to_string()),
        octocrab::Error::Other { source, .. } | octocrab::Error::Service { source, .. } => {
            (None, source.to_string())
        }
        _ => (None, "Unknown error".to_string()),
    }
}

/// Check if an error is a network error by walking the error chain. If it is, it might be a
/// retryable error.
fn is_network_error(e: &(dyn Error + 'static)) -> bool {
    let mut current = Some(e);

    while let Some(err) = current {
        if let Some(hyper_err) = err.downcast_ref::<hyper::Error>()
            && (hyper_err.is_closed()
                || hyper_err.is_incomplete_message()
                || hyper_err.is_timeout())
        {
            return true;
        }

        // reqwest errors (timeout, connect, etc.)
        if let Some(reqwest_err) = err.downcast_ref::<reqwest::Error>()
            && (reqwest_err.is_timeout() || reqwest_err.is_connect() || reqwest_err.is_request())
        {
            return true;
        }

        if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
            match io_err.kind() {
                TimedOut | ConnectionRefused | ConnectionAborted | ConnectionReset
                | NotConnected | AddrNotAvailable | AddrInUse | NetworkUnreachable
                | HostUnreachable | BrokenPipe | WouldBlock => return true,
                _ => {}
            }
        }

        // String-based fallback (last resort)
        let msg = err.to_string().to_lowercase();
        if msg.contains("dns error")
            || msg.contains("failed to lookup address")
            || msg.contains("nodename nor servname provided")
            || msg.contains("network unreachable")
            || msg.contains("no route to host")
            || msg.contains("connection refused")
            || msg.contains("timed out")
        {
            return true;
        }

        current = err.source();
    }
    false
}
