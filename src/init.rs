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
use std::env;
use std::fs;
use std::path::PathBuf;

use crate::config::CONFIG_NAME;
use crate::error::GitError;
/// A basic sample configuration that can be initialized
/// by using the `config` command
const SAMPLE_CONFIG: &str = r#"#Git repo syncing configuration
[github]
#token = "Add your github api token here"

# The 'git' section here doesn't currently do anything
[git]
#default_directory = "/path/to/repos"
#Use shallow clones by default to use less disk space,
#bandwidth, and decrease processing time
shallow_by_default = true
#owner = "set owner organization/user here"
#email = "email here"

# Needed if you want slack integration
# webhook_url should be an https address
[slack]
webhook_url = ""

# This can be used to change who is commiting changes and creating
# releases, instead of using the user who is already set up for git
# in the environment.
[info]
#name = "set the name of who is commiting changes"
#email = "set the email of who is commiting changes"

# All repositories should be an https url to the repository
# Any operation that needs an ssh url will be automatically
# converted.
[repos]
#fork = ["repo1", "repo2"]
# This is for forked repositories that do not have a configured parent repository.
# This is a map of {"forked_url": "real_upstream_url"}
#fork_with_workaround = {}
#private = ["repo1", "repo2"]
#public = []

[misc]
# A list of branches that should be ignored when performing
# a search of stale branches, AKA branches that should never be
# reported as being old, no matter when they were last updated.
branch_blacklist = ["main", "master"]

"#;

/// Create a sample config file
pub fn generate_config(path: Option<&PathBuf>, force: bool) -> Result<PathBuf, GitError> {
    // This is only needed if a path isn't provided
    let home_dir = dirs::home_dir();
    let config_path = if let Some(config) = home_dir {
        let config_dir = config.join(".config").join(CONFIG_NAME);
        if !config_dir.exists() {
            fs::create_dir(config_dir.clone())?;
        }
        config_dir
    } else {
        // This is the current directory where this tool is being run
        env::current_dir()
            .expect("Failed to get current directory")
            .join(CONFIG_NAME)
        //PathBuf::from(CONFIG_NAME)
    };
    match (path, force) {
        (Some(path), true) => {
            if path.exists() {
                let mut backup = path.clone().into_os_string();
                backup.push(".bak");
                fs::copy(path, backup)?;
            }
            fs::write(path, SAMPLE_CONFIG)?;
            Ok(path.clone())
        }
        (Some(path), false) => {
            if path.exists() {
                return Err(GitError::Other(format!(
                    "Config file already exists at {}. Use --force to overwrite",
                    path.display()
                )));
            }
            fs::write(path, SAMPLE_CONFIG)?;
            Ok(path.clone())
        }
        (None, true) => {
            if config_path.exists() {
                let mut backup = config_path.clone().into_os_string();
                backup.push(".bak");
                fs::copy(&config_path, backup)?;
            }
            fs::write(&config_path, SAMPLE_CONFIG)?;
            Ok(config_path)
        }
        (None, false) => {
            println!("{}", config_path.display());
            if config_path.exists() {
                return Err(GitError::Other(format!(
                    "Config file already exists at {}. Use --force to overwrite",
                    config_path.display()
                )));
            }
            fs::write(&config_path, SAMPLE_CONFIG)?;
            Ok(config_path)
        }
    }
    /*
    let home_dir = dirs::home_dir();
    if let Some(home) = home_dir {
        let config_path = home.join(".config").join(CONFIG_NAME);
        if config_path.exists() && !force {
            eprintln!(
                "Config file already exists at {}. Use --force to overwrite",
                config_path.display()
            );
            return Ok(config_path);
        }
    }
    let config_path = path.unwrap_or(PathBuf::from(CONFIG_NAME));
    if config_path.exists() && !force {
        eprintln!(
            "Config file already exists at {}. Use --force to overwrite",
            config_path.display()
        );
        return Ok(config_path);
    }

    if let Some(parent) = config_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }
    if config_path.exists() {
        let mut backup = config_path.clone().into_os_string();
        backup.push(".bak");
        fs::copy(&config_path, backup)?;
    }
    fs::write(&config_path, SAMPLE_CONFIG)?;
    println!("Config file created at {}", config_path.display());
    Ok(config_path)
    */
}
