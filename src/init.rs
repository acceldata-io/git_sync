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

    fs::write(&config_path, sample_config)?;
    println!("Config file created at {}", config_path.display());
    Ok(config_path)
}
