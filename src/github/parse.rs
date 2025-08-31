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
use crate::utils::pr::{CreatePrOptions, MergePrOptions};
use crate::utils::repo::RepoChecks;
use clap_complete::{
    generate_to,
    shells::{Bash, Fish, Zsh},
};
use clap_mangen::Man;
use console::user_attended;
use regex::Regex;
use std::fs::File;
use std::path::{Path, PathBuf};

/// Parse the argument that gets passed, and run their associated methods
pub async fn match_arguments(app: &AppArgs, config: Config) -> Result<(), GitError> {
    // If we're trying to generate man pages or shell completion, we don't need
    // a github token
    let token = match &app.command {
        Command::Generate { .. } | Command::Config { .. } => "".to_string(),
        _ => app
            .token
            .clone()
            .or_else(|| config.get_github_token().clone())
            .ok_or(GitError::MissingToken)?,
    };

    let repos = match app.repository_type {
        RepositoryType::Public => config.get_public_repositories(),
        RepositoryType::Private => config.get_private_repositories(),
        RepositoryType::Fork => config.get_fork_repositories(),
        RepositoryType::All => config.get_all_repositories(),
    };

    let is_user = user_attended();

    let client = GithubClient::new(token.to_string(), &config, is_user)?;
    if !token.is_empty() {
        let (rest_limit, graphql_limit) =
            tokio::join!(client.get_rate_limit(), client.get_graphql_limit());

        if rest_limit.is_err() {
            eprintln!("Warning: Could not fetch REST API rate limit");
        }
        if graphql_limit.is_err() {
            eprintln!("Warning: Could not fetch GraphQL API rate limit");
        }
    }
    match &app.command {
        Command::Tag { cmd } => match cmd {
            TagCommand::Compare(compare_cmd) => {
                compare_cmd.validate().map_err(GitError::Other)?;
                let repository = compare_cmd.repository.as_ref();
                if compare_cmd.all && !repos.is_empty() {
                    client.diff_all_tags(repos).await?
                } else if compare_cmd.all {
                    return Err(GitError::NoReposConfigured);
                } else if let Some(repository) = repository {
                    let _diffs = client.diff_tags(repository).await?;
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            }
            TagCommand::Create(create_cmd) => {
                let tag = &create_cmd.tag;
                let branch = &create_cmd.branch;
                let repository = create_cmd.repository.as_ref();

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
                let repository = delete_cmd.repository.as_ref();

                if delete_cmd.all {
                    client.delete_all_tags(&delete_cmd.tag, &repos).await?
                } else if let Some(repository) = repository {
                    client.delete_tag(repository, &delete_cmd.tag).await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            }
            TagCommand::Sync(sync_cmd) => {
                let repository = sync_cmd.repository.as_ref();
                // By default, this is false
                let process_annotated_tags = sync_cmd.without_annotated;
                if sync_cmd.all {
                    client.sync_all_tags(process_annotated_tags, repos).await?
                } else if let Some(repository) = repository {
                    client.sync_tags(repository, process_annotated_tags).await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            }
        },
        Command::Repo { cmd } => match cmd {
            RepoCommand::Sync(sync_cmd) => {
                let repository = sync_cmd.repository.as_ref();
                if sync_cmd.all {
                    client.sync_all_forks(repos).await?
                } else if let Some(repository) = repository {
                    client.sync_fork(repository).await?
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            }
            RepoCommand::Check(check_cmd) => {
                let repository = check_cmd.repository.as_ref();
                let (protected, license) = (check_cmd.protected, check_cmd.license);
                let old_branches = (check_cmd.old_branches, check_cmd.days_ago);
                let blacklist = config.misc.branch_blacklist.unwrap_or_default();
                let filter = check_cmd.branch_filter.clone();
                let branch_filter = if let Some(filter) = filter {
                    Some(Regex::new(&filter).map_err(GitError::RegexError)?)
                } else {
                    None
                };
                let checks = &RepoChecks {
                    protected,
                    license,
                    old_branches,
                    branch_filter: branch_filter.clone(),
                };

                if check_cmd.all {
                    let result = client
                        .check_all_repositories(repos, checks, &blacklist)
                        .await?;

                    for res in &result {
                        println!("{:?}", res.0);
                        let (branches, rules, license, repo) = res;
                        for branch in branches {
                            println!("\tStale branch: {} - {}", branch.0, branch.1);
                        }
                        for rule in rules {
                            rule.print(repo);
                        }
                        if let Some(license) = license {
                            if let Some(name) = license.name.as_ref() {
                                println!("\tLicense: {name}");
                            }
                        }
                    }
                } else if let Some(repository) = repository {
                    let result = client
                        .check_repository(repository, blacklist, checks)
                        .await?;
                    let (_, rules, license, repo) = &result;
                    for rule in rules {
                        rule.print(repo);
                    }
                    println!("License: {license:?}")
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            }
        },
        Command::Branch { cmd } => match cmd {
            BranchCommand::Create(create_cmd) => {
                let repository = create_cmd.repository.as_ref();
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
                let repository = delete_cmd.repository.as_ref();
                if delete_cmd.all {
                    client
                        .delete_all_branches(&delete_cmd.branch, &repos)
                        .await?
                } else if let Some(repository) = repository {
                    client.delete_branch(repository, &delete_cmd.branch).await?
                }
            }
        },
        Command::Release { cmd } => match cmd {
            ReleaseCommand::Create(create_cmd) => {
                let repository = create_cmd.repository.as_ref();
                if create_cmd.all {
                    {}
                } else if let Some(repository) = repository {
                    client
                        .create_release(
                            repository,
                            &create_cmd.current_release,
                            &create_cmd.previous_release,
                            create_cmd.release_name.as_deref(),
                        )
                        .await?;
                }
            }
        },
        Command::PR { cmd } => match cmd {
            PRCommand::Open(open_cmd) => {
                let repository = open_cmd.repository.clone().unwrap_or_default();
                let merge = open_cmd.merge;
                println!("Merge is {merge}");
                let opts = CreatePrOptions {
                    url: repository.clone(),
                    head: open_cmd.head.clone(),
                    base: open_cmd.base.clone(),
                    title: open_cmd.title.clone(),
                    body: open_cmd.body.clone(),
                    reviewers: open_cmd.reviewers.clone(),
                };

                let mut merge_opts = if merge {
                    Some(MergePrOptions {
                        url: repository.clone(),
                        pr_number: 0,
                        method: open_cmd.merge_method,
                        title: open_cmd.merge_title.clone(),
                        message: open_cmd.merge_body.clone(),
                        sha: open_cmd.sha.clone(),
                    })
                } else {
                    None
                };
                if open_cmd.all {
                    let pr_numbers = client.create_all_prs(&opts, merge_opts, repos).await?;
                } else if !repository.len() > 0 {
                    let pr_number = client.create_pr(&opts).await?;
                    println!("Created PR #{pr_number} in {repository}");
                    if let Some(opts) = merge_opts.as_mut() {
                        opts.pr_number = pr_number;
                        client.merge_pr(opts).await?;
                    }
                } else {
                    return Err(GitError::MissingRepositoryName);
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
                    println!("Generated bash completions");
                }
                "zsh" => {
                    generate_to(Zsh, &mut cmd, "git_sync", out_dir)?;
                    println!("Generated zsh completions");
                }
                "fish" => {
                    generate_to(Fish, &mut cmd, "git_sync", out_dir)?;
                    println!("Generated fish completions");
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
fn write_man(cmd: &mut clap::Command, out_dir: &Path, name: &str) {
    let man = Man::new(cmd.clone());
    let mut file = File::create(out_dir.join(name)).unwrap();
    man.render(&mut file).unwrap();
}

/// Generate manpages for all subcommands. Otherwise we only get a manpage for the root command.
fn generate_manpages(mut cmd: clap::Command, out_dir: &PathBuf, parent: Option<String>) {
    let name = if let Some(parent) = parent {
        format!("git_sync-{parent}.1")
    } else {
        "git_sync.1".to_string()
    };
    write_man(&mut cmd, out_dir, &name);

    for subcommand in cmd.get_subcommands() {
        if subcommand.is_hide_set() {
            continue;
        }
        let sub_name = subcommand.get_name().to_string().replace('_', "-");
        generate_manpages(subcommand.clone(), out_dir, Some(sub_name));
    }
}
