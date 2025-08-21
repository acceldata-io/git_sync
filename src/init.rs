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
use std::fs;
use std::path::PathBuf;

use crate::config::CONFIG_NAME;
use crate::error::GitError;

/// Create a sample config file
pub fn generate_config(path: Option<PathBuf>, force: bool) -> Result<PathBuf, GitError> {
    let config_path = path.unwrap_or(PathBuf::from(CONFIG_NAME));
    if config_path.exists() && !force {
        eprintln!(
            "Config file already exists at {}. Use --force to overwrite",
            config_path.display()
        );
        return Ok(config_path);
    }
    let sample_config = r#"#Git repo syncing configuration
[github]
#token = "Add your github api token here"

[git]
#default_directory = "/path/to/repos"
#Use shallow clones by default to use less disk space,
#bandwidth, and decrease processing time
shallow_by_default = true
#owner="set owner organization/user here"

[info]
#name="set the name of who is commiting changes"
#email="set the email of who is commiting changes"

#repos is just the name of a repository, not including the owner.
#ex: https://github.com/apache/hadoop/ -> just "hadoop"
[fork_repos]
#repos=["repo1","repo2"]
[private_repos]
#repos=["repo1", "repo2"]
"#;

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
    fs::write(&config_path, sample_config)?;
    println!("Config file created at {}", config_path.display());
    Ok(config_path)
}
