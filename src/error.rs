use thiserror::Error;

#[derive(Error, Debug)]
pub enum GitError {
    #[error("Multiple errors: {0:?}")]
    MultipleErrors(Vec<GitError>),
    #[error("Github API error: {0}")]
    GithubApiError(#[from] octocrab::Error),
    #[error("Upstream not found for fork")]
    NoUpstreamRepo,

    #[error("Invalid repository URL: {0}")]
    InvalidRepository(String),

    #[error("Regex error: {0}")]
    RegexError(#[from] regex::Error),

    #[error("Repository not found: {0}")]
    RepoNotFound(String),

    #[error("No repos configured")]
    NoReposConfigured,

    #[error("Failed to get repository info: {0}")]
    RepoInfoError(String),

    #[error("Repository is not a fork")]
    NotAFork,

    #[error("Missing repository name")]
    MissingRepositoryName,

    #[error("No such branch: {0}")]
    NoSuchBranch(String),

    #[error("No owner or repo specified")]
    NoOwnerOrRepo,

    #[error("TOML parsing error: {0}")]
    TomlError(#[from] toml::de::Error),

    #[error("License missing for {0}")]
    MissingLicense(String),

    #[error("No main branch protection for {0}")]
    NoMainBranchProtection(String),

    #[error(
        "Missing GitHub token. Please provide a token via --token, GITHUB_TOKEN environment variable, or in your config file."
    )]
    MissingToken,

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Error: {0}")]
    Other(String),
}


