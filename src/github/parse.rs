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
use crate::GitError;
use crate::cli::*;
use crate::config::Config;
use crate::github::client::GithubClient;
use crate::init::generate_config;
use crate::utils::repo::RepoChecks;
use clap_complete::{
    generate_to,
    shells::{Bash, Fish, Zsh},
};
use clap_mangen::Man;
use regex::Regex;
use std::fs::File;
use std::path::PathBuf;

/// Parse the argument that gets passed, and run their associated methods
pub async fn match_arguments(app: &AppArgs, config: Config) -> Result<(), GitError> {
    let token = app
        .token
        .clone()
        .or_else(|| config.get_github_token().clone())
        .ok_or(GitError::MissingToken)?;

    let repos = match app.repository_type {
        RepositoryType::Public => config.get_public_repositories(),
        RepositoryType::Private => config.get_private_repositories(),
        RepositoryType::Fork => config.get_fork_repositories(),
        RepositoryType::All => config.get_all_repositories(),
    };
    let client = GithubClient::new(token.to_string())?;

    println!("{repos:#?}");
    match &app.command {
        Command::Tag { cmd, repository } => match cmd {
            TagCommand::Compare(compare_cmd) => {
                compare_cmd.validate().map_err(GitError::Other)?;

                if compare_cmd.all && !repos.is_empty() {
                    client.diff_all_tags(repos).await?
                } else if compare_cmd.all {
                    return Err(GitError::NoReposConfigured);
                } else if let Some(repository) = repository {
                    client.diff_tags(repository).await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            }
            TagCommand::Create(create_cmd) => {
                let tag = &create_cmd.tag;
                let branch = &create_cmd.branch;
                if create_cmd.all {
                    client.create_all_tags(tag, branch, repos).await?
                } else if let Some(repository) = repository {
                    client
                        .create_tag(repository, &create_cmd.tag, &create_cmd.branch)
                        .await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            }
            TagCommand::Delete(delete_cmd) => {
                if delete_cmd.all {
                    client.delete_all_tags(&delete_cmd.tag, &repos).await?
                } else if let Some(repository) = repository {
                    client.delete_tag(repository, &delete_cmd.tag).await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            }
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
            }
            RepoCommand::Check(check_cmd) => {
                let (protected, license) = (check_cmd.protected, check_cmd.license);
                let old_branches = (check_cmd.old_branches, check_cmd.days_ago);
                let blacklist = config.misc.branch_blacklist.unwrap_or_default();
                let filter = check_cmd.branch_filter.clone();
                let branch_filter = if let Some(filter) = filter {
                    Some(Regex::new(&filter).map_err(GitError::RegexError)?)
                } else {
                    None
                };
                if check_cmd.all {
                    client
                        .check_all_repositories(
                            repos,
                            &RepoChecks {
                                protected,
                                license,
                                old_branches,
                                branch_filter,
                            },
                            &blacklist,
                        )
                        .await?
                } else if let Some(repository) = repository {
                    client
                        .check_repository(
                            repository,
                            &RepoChecks {
                                protected,
                                license,
                                old_branches,
                                branch_filter,
                            },
                            &blacklist,
                        )
                        .await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            }
        },
        Command::Branch { cmd, repository } => match cmd {
            BranchCommand::Create(create_cmd) => {
                let (base, new) = (
                    create_cmd.base_branch.clone(),
                    create_cmd.new_branch.clone(),
                );
                if create_cmd.all {
                    client.create_all_branches(&base, &new, &repos).await?
                } else if let Some(repository) = repository {
                    client.create_branch(repository, &base, &new).await?
                }
            }
            BranchCommand::Delete(delete_cmd) => {
                if delete_cmd.all {
                    client
                        .delete_all_branches(&delete_cmd.branch, &repos)
                        .await?
                } else if let Some(repository) = repository {
                    client.delete_branch(repository, &delete_cmd.branch).await?
                }
            }
        },
        Command::Release { repository, cmd } => match cmd {
            ReleaseCommand::Create(create_cmd) => {
                if create_cmd.all {
                    {}
                } else if let Some(repository) = repository {
                    let out = client
                        .generate_release_notes(
                            repository,
                            &create_cmd.base,
                            &create_cmd.previous_release,
                        )
                        .await;
                    println!("{out:#?}");
                }
            }
        },
        Command::Config { file, force } => {
            generate_config(file, *force)?;
        }
        Command::Generate { kind, out } => {
            let mut cmd = cli();

            let out_dir = out
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap());
            match kind.as_str() {
                "bash" => {
                    generate_to(Bash, &mut cmd, "git_sync", out_dir)?;
                }
                "zsh" => {
                    generate_to(Zsh, &mut cmd, "git_sync", out_dir)?;
                }
                "fish" => {
                    generate_to(Fish, &mut cmd, "git_sync", out_dir)?;
                }
                "man" => {
                    let out_dir = out
                        .clone()
                        .unwrap_or_else(|| std::env::current_dir().unwrap());
                    let cmd = cli();
                    generate_manpages(cmd, &out_dir, None);
                    println!("Manpages generated");
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Helper to write manpages
fn write_man(cmd: &mut clap::Command, out_dir: &PathBuf, name: &str) {
    let man = Man::new(cmd.clone());
    let mut file = File::create(out_dir.join(name)).unwrap();
    man.render(&mut file).unwrap();
}

/// Generate manpages for all subcommands
fn generate_manpages(mut cmd: clap::Command, out_dir: &PathBuf, parent: Option<String>) {
    let name = if let Some(parent) = parent {
        format!("git_sync-{parent}.1")
    } else {
        "git_sync.1".to_string()
    };
    write_man(&mut cmd, out_dir, &name);

    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() {
            continue;
        }
        let sub_name = sub.get_name().to_string().replace('_', "-");
        generate_manpages(sub.clone(), out_dir, Some(sub_name));
    }
}
