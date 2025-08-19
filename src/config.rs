use crate::error::GitError;
use crate::utils::user::UserDetails;
use dirs::home_dir;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

pub const CONFIG_NAME: &str = "git-manage.toml";


/// This is the root level of the configuration file
#[derive(Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub github: GithubConfig,

    #[serde(default)]
    pub git: GitConfig,

    #[serde(default)]
    pub repos: RepoConfig,

    #[serde(default)]
    pub info: InfoConfig,
}

#[derive(Debug, Deserialize, Default)]
pub struct GithubConfig {
    pub token: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct GitConfig {
    pub default_directory: Option<PathBuf>,
    pub shallow_by_default: Option<bool>,
    pub owner: Option<String>,
}
#[derive(Debug, Deserialize, Default)]
pub struct InfoConfig {
    pub name: Option<String>,
    pub email: Option<String>,
}
#[derive(Debug, Deserialize, Default)]
pub struct RepoConfig {
    pub fork: Option<Vec<String>>,
    pub private: Option<Vec<String>>,
    pub public: Option<Vec<String>>,
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
        eprintln!("Unable to find {CONFIG_NAME}, setting values to their default");
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
    /// Get git information about the current user, first from the environment,
    /// then from the toml configuration, and as a last resort, from git itself.
    pub fn get_user_info(&self) -> Option<UserDetails> {

        let name = if let Ok(name) = std::env::var("GIT_USER") {
            Some(name)
        } else {
            self.info.name.clone()
        };
        let email= if let Ok(email) = std::env::var("GIT_EMAIL") {
            Some(email)
        } else {
            self.info.email.clone()
        };

        match (name, email) {
            (Some(name), Some(email)) => Some(UserDetails::new(name, email)),
            _ => {
                UserDetails::new_from_git().ok()
            }
        }
    }

    /// Get the repo owner
    pub fn get_owner(&self) -> Option<String> {
        if let Ok(name) = std::env::var("GIT_OWNER") {
            return Some(name);
        }
        self.git.owner.clone()
    }
    /// Get a vector of repositories defined in the config file that are forks
    pub fn get_fork_repositories(&self) -> Option<Vec<String>> {
        self.repos.fork.clone()
    }
    /// Get a vector of private repositories defined in the config file
    pub fn get_private_repositories(&self) -> Option<Vec<String>> {
        self.repos.private.clone()
    }
    pub fn get_public_repositories(&self) -> Option<Vec<String>> {
        self.repos.public.clone()

    }}
