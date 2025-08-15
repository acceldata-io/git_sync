mod cli;
mod config;
mod error;
mod github;
mod init;

use cli::{Command, CompareCommand, parse_args};
use config::Config;
use error::GitError;
use github::{diff_tags, diff_tags_all};
use octocrab::Octocrab;
use std::error::Error;

use init::generate_config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args();

    if let Command::InitConfig { path, force } = &args.command {
        let path = generate_config(path.clone(), *force)?;
        println!("Config created at {}", path.display());
        return Ok(());
    }

    let config = if let Some(config_path) = &args.file {
        Config::from_file(config_path)?
    } else {
        Config::load()
    };

    let token = args
        .token
        .or_else(|| config.get_github_token())
        .ok_or(GitError::MissingToken)?;

    let repos = config.get_repositories().unwrap_or_default();
    println!("{repos:?}");

    let octocrab = Octocrab::builder()
        .personal_token(token)
        .build()
        .map_err(GitError::GithubApiError)?;
    /*let repos = vec![
        ("acceldata-io", "kudu"),
        ("acceldata-io", "hive"),
        ("acceldata-io", "hadoop"),
        ("acceldata-io", "airflow"),
    ];*/
    match &args.command {
        Command::Compare { cmd } => match cmd {
            CompareCommand::Single { owner, repo } => diff_tags(&octocrab, owner, repo).await,
            CompareCommand::All => diff_tags_all(&octocrab, repos).await,
        },
        Command::InitConfig { .. } => Ok(()),
    }?;

    Ok(())
}
