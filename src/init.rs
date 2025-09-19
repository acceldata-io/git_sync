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
use crate::config::CONFIG_NAME;
use crate::error::GitError;
use microxdg::Xdg;
use std::fs;
use std::path::PathBuf;
/// A basic sample configuration that can be initialized
/// by using the `config` command
const SAMPLE_CONFIG: &str = r#"#Git repo syncing configuration
[github]
#token = "Add your github api token here"

# Needed if you want slack integration
# webhook_url should be an https address
[slack]
# Eventually, the enabled option will be added to always enable slack for each run
#enable=true
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
# A list of licenses that you will be warned about. 
# You must use their spdx id
# A list of many of these can be found at https://spdx.org/licenses/
license_blacklist = [
    "CC-BY-NC-4.0", 
    "GPL-3.0-or-later", 
    "GPL-3.0-only", 
    "GPL-2.0-or-later"
    "GPL-2.0-only",
]
"#;

/// Create a sample config file
pub fn generate_config(path: Option<&PathBuf>, force: bool) -> Result<PathBuf, GitError> {
    // This is only needed if a path isn't provided
    let xdg = Xdg::new();
    let config_path = if let Ok(xdg) = xdg {
        let f = xdg.config_file(CONFIG_NAME);
        f.ok()
    } else {
        None
    };

    match (path, force, config_path) {
        (Some(path), true, _) => {
            if path.exists() {
                let mut backup = path.clone().into_os_string();
                backup.push(".bak");
                fs::copy(path, backup)?;
            }
            fs::write(path, SAMPLE_CONFIG)?;
            Ok(path.clone())
        }
        (Some(path), false, _) => {
            if path.exists() {
                return Err(GitError::Other(format!(
                    "Config file already exists at {}. Use --force to overwrite",
                    path.display()
                )));
            }
            fs::write(path, SAMPLE_CONFIG)?;
            Ok(path.clone())
        }
        (None, true, Some(config_path)) => {
            if config_path.exists() {
                let mut backup = config_path.clone().into_os_string();
                backup.push(".bak");
                fs::copy(&config_path, backup)?;
            }
            fs::write(&config_path, SAMPLE_CONFIG)?;
            Ok(config_path)
        }
        (None, false, Some(config_path)) => {
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
        (None, _, None) => {
            panic!(
                "No config file specified and $XDG_CONFIG_HOME or $HOME/.config could not be determined. Cannot continue"
            );
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
