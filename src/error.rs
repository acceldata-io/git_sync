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
    #[error("Multiple errors: {0:?}")]
    MultipleErrors(Vec<(String, GitError)>),
    #[error("Github API error: {0}")]
    GithubApiError(#[from] octocrab::Error),
    #[error("PR #{0} not mergeable")]
    PRNotMergeable(u64),
    #[error("Upstream not found for fork")]
    NoUpstreamRepo,

    #[error("Date parse error: {0}")]
    DateParseError(#[from] chrono::ParseError),

    #[error("Invalid repository URL: {0}")]
    InvalidRepository(String),

    #[error("Regex error: {0}")]
    RegexError(#[from] fancy_regex::Error),

    #[error("No repos configured")]
    NoReposConfigured,

    #[error("Repository is not a fork")]
    NotAFork,

    #[error("Missing repository name")]
    MissingRepositoryName,

    #[error("No such branch: {0}")]
    NoSuchBranch(String),
    #[allow(dead_code)]
    #[error("No such tag: {0}")]
    NoSuchTag(String),

    #[error("TOML parsing error: {0}")]
    TomlError(#[from] toml::de::Error),

    #[error(
        "Missing GitHub token. Please provide a token via --token, GITHUB_TOKEN environment variable, or in your config file."
    )]
    MissingToken,
    #[error("Join error: {0}")]
    JoinError(#[from] tokio::task::JoinError),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("Timezone error: {0}")]
    GetTimezoneError(#[from] iana_time_zone::GetTimezoneError),

    #[error("Semaphore acquire failure: {0}")]
    SemaphoreError(#[from] tokio::sync::AcquireError),

    #[error("Error: {0}")]
    Other(String),
}

impl GitError {
    /// Convert this error into a user-friendly version
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
            GitError::PRNotMergeable(pr_number) => UserError {
                code: None,
                message: format!("PR #{pr_number} cannot be merged automatically"),
                suggestion: Some("Human intervention may be required.".into()),
            },
            GitError::NoUpstreamRepo => UserError {
                code: None,
                message: "Could not find an upstream repository for this fork.".into(),
                suggestion: Some("Make sure the repository is a fork and has an upstream set.".into()),
            },
            GitError::DateParseError(e) => UserError {
                code: None,
                message: format!("Invalid date: {e}"),
                suggestion: Some("Use a valid date format.".into()),
            },
            GitError::InvalidRepository(url) => UserError {
                code: None,
                message: format!("Invalid repository URL: {url}"),
                suggestion: Some("Check the repository URL.".into()),
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
            if let Some(ioe) = err.downcast_ref::<std::io::Error>() {
                if ioe.kind() == std::io::ErrorKind::BrokenPipe {
                    return true;
                }
            }
            source = err.source();
        }
        false
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

/// Check if whatever error is passed is reasonably retryable. Most errors aren't.
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
                    if let Some(obj) = err.as_object() {
                        if let Some(msg) = obj.get("message").and_then(|m| m.as_str()) {
                            let m = format!("\n- {msg}");
                            let _ = write!(message, "{m}");
                        }
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

fn is_network_error(e: &(dyn Error + 'static)) -> bool {
    let mut current = Some(e);

    while let Some(err) = current {
        if let Some(hyper_err) = err.downcast_ref::<hyper::Error>() {
            if hyper_err.is_closed() || hyper_err.is_incomplete_message() || hyper_err.is_timeout()
            {
                return true;
            }
        }

        // reqwest errors (timeout, connect, etc.)
        if let Some(reqwest_err) = err.downcast_ref::<reqwest::Error>() {
            if reqwest_err.is_timeout() || reqwest_err.is_connect() || reqwest_err.is_request() {
                return true;
            }
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
