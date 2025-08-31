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
use crate::utils::user::UserDetails;
use dirs::home_dir;
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The currently defined name of the config file.
/// This may change in the future.
pub const CONFIG_NAME: &str = "git-manage.toml";

/// This is the root level of the configuration file
#[derive(Default, Deserialize, Debug)]
pub struct Config {
    #[serde(default)]
    pub github: GithubConfig,

    #[allow(dead_code)]
    #[serde(default)]
    pub git: GitConfig,

    #[serde(default)]
    pub repos: RepoConfig,

    #[allow(dead_code)]
    #[serde(default)]
    pub info: InfoConfig,

    #[serde(default)]
    pub misc: MiscSettings,
}

#[derive(Debug, Deserialize, Default)]
pub struct MiscSettings {
    pub branch_blacklist: Option<HashSet<String>>,
}

/// Github configuration struct
#[derive(Debug, Deserialize, Default)]
pub struct GithubConfig {
    pub token: Option<String>,
}

/// Options for git. This may be removed
#[allow(dead_code)]
#[derive(Deserialize, Default, Debug)]
pub struct GitConfig {
    pub default_directory: Option<PathBuf>,
    pub shallow_by_default: Option<bool>,
    pub owner: Option<String>,
}
/// This should probably be moved to git config
#[allow(dead_code)]
#[derive(Debug, Deserialize, Default)]
pub struct InfoConfig {
    pub name: Option<String>,
    pub email: Option<String>,
}
/// The configuration for the different types of repositories
#[derive(Debug, Deserialize, Default)]
pub struct RepoConfig {
    pub fork: Option<Vec<String>>,
    pub private: Option<Vec<String>>,
    pub public: Option<Vec<String>>,
}

impl Config {
    /// Create a new config from an optional path. If None, then it tries to load the config.
    /// If it can't be loaded, the default values are loaded, which probably won't work as
    /// expected.
    pub fn new(path: &Option<PathBuf>) -> Result<Self, GitError> {
        if let Some(config_path) = path {
            Ok(Config::from_file(config_path)?)
        } else {
            Ok(Config::load())
        }
    }
    /// Load config from an existing toml file.
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
    /// Try to load the config from expected locations. Checks in the current directory first,
    /// then checks for the configuration file in ~/.config/ next. If all else fails, it sets
    /// everything to the default.
    /// In the default case, as long as you have the GITHUB_TOKEN defined as an environment
    /// variable, you can still perform operations through the github api.
    /// If not, this tool will fail to run and tell you you're missing the token.
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
    #[allow(dead_code)]
    pub fn get_user_info(&self) -> Option<UserDetails> {
        let name = if let Ok(name) = std::env::var("GIT_USER") {
            Some(name)
        } else {
            self.info.name.clone()
        };
        let email = if let Ok(email) = std::env::var("GIT_EMAIL") {
            Some(email)
        } else {
            self.info.email.clone()
        };

        match (name, email) {
            (Some(name), Some(email)) => Some(UserDetails::new(name, email)),
            _ => UserDetails::new_from_git().ok(),
        }
    }

    /// Get the repo owner
    #[allow(dead_code)]
    pub fn get_owner(&self) -> Option<String> {
        if let Ok(name) = std::env::var("GIT_OWNER") {
            return Some(name);
        }
        self.git.owner.clone()
    }
    /// Get a vector of repositories defined in the config file that are forks
    pub fn get_fork_repositories(&self) -> Vec<String> {
        println!("{:?}", self.repos.fork.clone());
        self.repos.fork.clone().unwrap_or_default()
    }
    /// Get a vector of private repositories defined in the config file
    pub fn get_private_repositories(&self) -> Vec<String> {
        self.repos.private.clone().unwrap_or_default()
    }
    pub fn get_public_repositories(&self) -> Vec<String> {
        self.repos.public.clone().unwrap_or_default()
    }
    pub fn get_all_repositories(&self) -> Vec<String> {
        self.repos
            .public
            .clone()
            .unwrap_or_default()
            .into_iter()
            .chain(self.get_fork_repositories())
            .chain(self.get_private_repositories())
            .collect()
    }
}
