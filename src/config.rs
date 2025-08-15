use crate::error::GitError;
use dirs::home_dir;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

pub const CONFIG_NAME: &str = "git-manage.toml";

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub github: GithubConfig,

    #[serde(default)]
    pub git: GitConfig,

    #[serde(default)]
    pub repos: RepoConfig,
}

#[derive(Debug, Deserialize, Default)]
pub struct GithubConfig {
    pub token: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct GitConfig {
    pub default_directory: Option<PathBuf>,
    pub shallow_by_default: Option<bool>,
    pub owner: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct RepoConfig {
    pub fork_repos: Repo,
    pub private_repos: Repo,
}
#[derive(Debug, Deserialize, Default)]
pub struct Repo {
    pub repos: Option<Vec<String>>,
}

impl Config {
    /// Load config from a toml file
    pub fn from_file(path: &Path) -> Result<Self, GitError> {
        let config_str = fs::read_to_string(path)?;
        match toml::from_str(&config_str) {
            Ok(config) => Ok(config),
            Err(e) => {
                eprintln!("Toml parse error in {path:?}: {e}");
                Err(GitError::TomlError(e))
            }
        }
    }
    /// Try to load the config from expected locations
    pub fn load() -> Self {
        if let Ok(local_config) = Config::from_file(Path::new(CONFIG_NAME)) {
            println!("Checking local...");
            return local_config;
        }
        if let Some(home_dir) = home_dir() {
            let home_config = home_dir.join(".config").join(CONFIG_NAME);
            println!("Checking home_dir: {home_config:?}");
            if let Ok(home_config) = Config::from_file(&home_config) {
                return home_config;
            }
        }
        // Config not found, return the default
        eprintln!("Unable to find {}, setting default", CONFIG_NAME);
        Config::default()
    }

    /// Get a github api token
    pub fn get_github_token(&self) -> Option<String> {
        // Check for the env variable GITHUB_TOKEN
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            return Some(token);
        }

        // Use the config file value if the above isn't defined
        self.github.token.clone()
    }

    /// Get the repo owner
    pub fn get_owner(&self) -> Option<String> {
        if let Ok(name) = std::env::var("GIT_OWNER") {
            return Some(name);
        }
        println!("{:?}", self.git.owner.clone());
        self.git.owner.clone()
    }

    pub fn get_repositories(&self) -> Option<Vec<(String, String)>> {
        let repo_list = self.repos.fork_repos.repos.as_ref()?;
        let owner = self.get_owner()?;

        Some(
            repo_list
                .iter()
                .map(|repo| (owner.to_string(), repo.to_string()))
                .collect(),
        )
    }
}
