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
use crate::cli::*;
use crate::GitError;
use crate::github::client::GithubClient;
use crate::init::generate_config;
use crate::utils::repo::RepoChecks;
use crate::config::Config;
use chrono::{DateTime,Utc};
use octocrab::params::repos::Reference;

/// Parse the argument that gets passed, and run their associated methods
pub async fn match_arguments(command: &Command, token: &str, config: Config) -> Result<(), GitError> {
    let repositories = config.get_fork_repositories();
    let client = GithubClient::new(token.to_string())?;

    let res = client.generate_release_notes("https://github.com/acceldata-io/kudu/", "ODP-3.3.6.2-1-tag", "ODP-3.3.6.1-1-tag").await?;
    println!("{res:#?}");
    /*for r in &c {
        let date = r.commit.committer.as_ref().map(|c| &c.date).unwrap().unwrap();
        println!("{} ", date.to_rfc3339());

        let lines = r.commit.message.lines().collect::<Vec<&str>>();
        if lines.len() > 1 {
            println!("* {}", lines.first().unwrap());
        } else {
            println!("* {}", r.commit.message);
        }
    }
    */
    /*
    let t = DateTime::parse_from_rfc3339("2025-05-12T00:00:00Z").unwrap().with_timezone(&Utc);
    let commits = client.octocrab.clone().repos("acceldata-io", "kudu")
        .list_commits()
        .branch("ODP-3.3.6.2-1-tag")
        .since(t)
        .per_page(100)
        .send().await?;

    for c in commits {
        let lines:Vec<&str> = c.commit.message.lines().collect();
        if lines.len() > 1 {
            println!("* {}", lines.first().unwrap());
        } else {
            println!("* {}", c.commit.message);

        }
    }
    */


    //let note = client.generate_release_notes("https://github.com/acceldata-io/kudu/", "ODP-3.3.6.2-1-tag", "ODP-3.3.6.1-1-tag").await?;

    /*let abc = client.octocrab.clone().repos("acceldata-io","kudu")
        .releases()
        .generate_release_notes("3.3.6.2-1006-tag").send().await?;

    let lines: Vec<&str>= abc.body.lines().map(|l| {
        match l.find(" by @"){
            Some(idx) => &l[..idx],
            None => l,
        }})
        .filter(|l| !l.starts_with("**Full Changelog**"))
        .collect::<Vec<&str>>();
    println!("{lines:?}");
    */
    //println!("{}\n{}", note.name, note.body);
    let repos: Vec<String> = repositories.unwrap_or_default();
    match &command {
        Command::Tag { cmd, repository} => match cmd {
            TagCommand::Compare(compare_cmd)=> {
                compare_cmd.validate().map_err(GitError::Other)?;
 
                if compare_cmd.all && !repos.is_empty() {
                    client.diff_all_tags(repos).await?

                } else if compare_cmd.all {
                    return Err(GitError::NoReposConfigured);
                } else if let Some(repository) = repository{
                    client.diff_tags(repository).await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            },
            TagCommand::Create(create_cmd) => {
                let tag = &create_cmd.tag;
                let branch = &create_cmd.branch;
                if create_cmd.all {
                    client.create_all_tags(tag, branch, repos).await?
                } else if let Some(repository) = repository {
                    client.create_tag(repository, &create_cmd.tag, &create_cmd.branch).await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            },
            TagCommand::Delete(delete_cmd) => {
                if delete_cmd.all {
                    client.delete_all_tags(&delete_cmd.tag, &repos).await?
                } else if let Some(repository) = repository {
                    client.delete_tag(repository, &delete_cmd.tag).await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            },
        },
        Command::Repo { cmd, repository } => match cmd {
            RepoCommand::Sync(sync_cmd) => {
                if sync_cmd.all {
                    client.sync_all_forks(repos).await?
                } else if let Some(repository) = repository {
                    client.sync_fork(repository).await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            },
            RepoCommand::Check(check_cmd) => {
                let (protected, license) = (check_cmd.protected, check_cmd.license);
                if check_cmd.all {
                    client.check_all_repositories(repos, &RepoChecks{protected, license}).await?
                } else if let Some(repository) = repository {
                    client.check_repository(repository, &RepoChecks{protected, license}).await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            }
        },
        Command::Branch{cmd, repository} => match cmd {
            BranchCommand::Create(create_cmd) => {
                let (base, new) = (create_cmd.base_branch.clone(), create_cmd.new_branch.clone());
                if create_cmd.all {
                    client.create_all_branches(&base, &new,&repos).await?
                } else if let Some(repository) = repository {
                    client.create_branch(repository, &base, &new ).await?
                }
            },
            BranchCommand::Delete(delete_cmd) => {
                if delete_cmd.all {
                    client.delete_all_branches(&delete_cmd.branch, &repos).await?
                } else if let Some(repository) = repository {
                    client.delete_branch(repository, &delete_cmd.branch).await?
                }
            },
        },
        Command::Config { path, force } => {
            generate_config(path.clone(), *force)?;
        },

    }
    Ok(())
}
