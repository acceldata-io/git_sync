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
    #[error("No such tag: {0}")]
    NoSuchTag(String),

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
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Error: {0}")]
    Other(String),
}
