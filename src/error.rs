use thiserror::Error;

#[derive(Error, Debug)]
pub enum GitError {
    #[error("Github API error: {0}")]
    GithubApiError(#[from] octocrab::Error),
    #[error("Upstream not found for fork")]
    NoUpstreamRepo,

    #[error("Failed to get repository info: {0}")]
    RepoInfoError(String),

    #[error("Repository is not a fork")]
    NotAFork,

    #[error("Missing parent repository owner info")]
    MissingParentOwner,

    #[error("Missing owner for forked repos")]
    MissingOwner,

    #[error("TOML parsing error: {0}")]
    TomlError(#[from] toml::de::Error),

    #[error(
        "Missing GitHub token. Please provide a token via --token, GITHUB_TOKEN environment variable, or in your config file."
    )]
    MissingToken,

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/*impl From<GitError> for Box<dyn std::error::Error> {
    fn from(err: GitError) -> Self {
        Box::new(err)
    }
}*/
