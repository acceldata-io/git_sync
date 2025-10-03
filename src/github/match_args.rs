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
use crate::cli::{
    AppArgs, BackupCommand, BackupDestination, BranchCommand, Command, PRCommand, ReleaseCommand,
    RepoCommand, RepositoryType, TagCommand, cli,
};
use crate::config::Config;
use crate::github::client::GithubClient;
use crate::init::generate_config;
use crate::utils::pr::{CreatePrOptions, MergePrOptions};
use crate::utils::repo::Checks;
use crate::utils::repo::RepoChecks;

use clap_complete::{
    generate_to,
    shells::{Bash, Fish, Zsh},
};
use clap_mangen::Man;
use fancy_regex::Regex;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::File;
use std::path::{Path, PathBuf};

/// Parse the argument that gets passed, and run their associated methods
pub async fn match_arguments(app: &AppArgs, config: Config) -> Result<(), GitError> {
    // If we're trying to generate man pages or shell completion, we don't need
    // a GitHub token
    let token = match &app.command {
        Command::Generate { .. } | Command::Config { .. } => String::new(),
        _ => app
            .token
            .clone()
            .or_else(|| config.get_github_token().clone())
            .ok_or(GitError::MissingToken)?,
    };
    let verbose = app.verbose;
    let quiet = app.quiet;
    let dry_run = app.dry_run;

    let fork_workaround_repositories = if app.with_fork_workaround {
        config.get_fork_workaround_repositories()
    } else {
        HashMap::new()
    };

    let repos = match app.repository_type {
        RepositoryType::Public => config.get_public_repositories(),
        RepositoryType::Private => config.get_private_repositories(),
        RepositoryType::Fork => config.get_fork_repositories(),
        RepositoryType::All => config.get_all_repositories(),
        RepositoryType::Custom => {
            let name = app.repository_group.clone().unwrap_or_default();
            let repository: Option<Vec<String>> = config
                .repos
                .custom
                .get(&app.repository_group.clone().unwrap_or_default())
                .cloned();
            if let Some(repository) = &repository {
                if repository.is_empty() {
                    return Err(GitError::Other(format!(
                        "No repositories found for custom group '{name}'"
                    )));
                }
                repository.clone()
            } else {
                return Err(GitError::Other(format!("Custom group '{name}' not found")));
            }
        }
    };
    // Remove any duplicate repositories. This shouldn't have any meaningful performance impact
    // since there won't ever be thousands of repositories configured.
    // Collect it into a Vec so that we have a consistent ordered collection
    let repos = repos
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    // If for some reason we cannot get the number of threads and the user doesn't try to define it,
    // default to 4, which seems like a reasonable minimum expectation for most systems.
    let default_jobs = std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(4);

    // Prioritize the command line argument or env variable over the config file
    let slack_webhook = app
        .slack_webhook
        .clone()
        .or_else(|| config.slack.webhook_url.clone());

    // This value must be greater than 0
    let jobs: usize = std::cmp::min(app.jobs.unwrap_or(default_jobs), 1);

    let client = GithubClient::new(&token, &config, jobs, slack_webhook)?;
    if !token.is_empty() && verbose {
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
        Command::Tag { cmd } => {
            match_tag_cmds(&client, repos, cmd, fork_workaround_repositories).await?;
        }
        Command::Repo { cmd } => {
            match_repo_cmds(&client, repos, config, cmd, fork_workaround_repositories).await?;
        }
        Command::Branch { cmd } => match_branch_cmds(&client, repos, cmd, quiet, dry_run).await?,
        Command::Release { cmd } => match_release_cmds(&client, repos, config, cmd).await?,
        Command::PR { cmd } => match_pr_cmds(&client, repos, config, cmd).await?,
        Command::Backup { cmd } => {
            match_backup_cmds(&client, repos, config, cmd, fork_workaround_repositories).await?;
        }
        Command::Config { file, force } => {
            generate_config(file.as_ref(), *force)?;
        }
        Command::Generate { kind, out } => {
            let mut cmd = cli();

            let out_dir = out.clone().unwrap_or_else(|| env::current_dir().unwrap());
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
                    let out_dir = out.clone().unwrap_or_else(|| env::current_dir().unwrap());
                    let cmd = cli();
                    generate_man_pages(cmd, &out_dir, None);
                    println!("Man pages generated");
                }
                _ => {}
            }
        }
    }
    // despite collecting the messages and errors always, we only actually send them to slack if
    // it's enabled.
    #[cfg(feature = "slack")]
    if app.slack {
        client.slack_message().await;
    }
    Ok(())
}

/// Process all Tag commands
async fn match_tag_cmds(
    client: &GithubClient,
    repos: Vec<String>,
    cmd: &TagCommand,
    fork_workaround: HashMap<String, String>,
) -> Result<(), GitError> {
    let result = async {
        match cmd {
            TagCommand::Compare(compare_cmd) => {
                let repository = compare_cmd.repository.as_ref();
                if compare_cmd.all && !repos.is_empty() {
                    client.diff_all_tags(repos).await?;
                } else if compare_cmd.all && repos.is_empty() {
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
                    client.create_all_tags(tag, branch, &repos[..]).await?;
                } else if let Some(repository) = repository {
                    client
                        .create_tag(repository, &create_cmd.tag, &create_cmd.branch)
                        .await?;
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            }
            TagCommand::Delete(delete_cmd) => {
                let repository = delete_cmd.repository.as_ref();

                if delete_cmd.all {
                    client.delete_all_tags(&delete_cmd.tag, &repos).await?;
                } else if let Some(repository) = repository {
                    client.delete_tag(repository, &delete_cmd.tag).await?;
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            }
            TagCommand::Sync(sync_cmd) => {
                let repository = sync_cmd.repository.as_ref();
                // By default, this is false
                let process_annotated_tags = sync_cmd.with_annotated;

                if sync_cmd.all {
                    client
                        .sync_all_tags(process_annotated_tags, &repos[..], fork_workaround)
                        .await?;
                } else if let Some(repository) = repository {
                    if let Some(parent) = fork_workaround.get(repository) {
                        client
                            .sync_tags(repository, Some(parent), process_annotated_tags)
                            .await?;
                    } else {
                        return Err(GitError::NoUpstreamRepo);
                    }
                } else {
                    return Err(GitError::MissingRepositoryName);
                }
            }
        }
        Ok(())
    }
    .await;

    if let Err(e) = result {
        client.slack_message().await;
        return Err(e);
    }
    Ok(())
}
/// Process all Branch commands
async fn match_branch_cmds(
    client: &GithubClient,
    repos: Vec<String>,
    cmd: &BranchCommand,
    quiet: bool,
    dry_run: bool,
) -> Result<(), GitError> {
    match cmd {
        BranchCommand::Create(create_cmd) => {
            let repository = create_cmd.repository.as_ref();
            let (base_branch, base_tag, new) = (
                create_cmd.base_branch.clone(),
                create_cmd.base_tag.clone(),
                create_cmd.new_branch.clone(),
            );
            if let Some(base) = &base_branch {
                if create_cmd.all {
                    client
                        .create_all_branches(base, &String::new(), &new, &repos[..], quiet)
                        .await?;
                } else if let Some(repository) = repository {
                    client.create_branch(repository, &base, &new, quiet).await?;
                }
            } else if let Some(tag) = &base_tag {
                if create_cmd.all {
                    client
                        .create_all_branches(&String::new(), tag, &new, &repos[..], quiet)
                        .await?;
                } else if let Some(repository) = repository {
                    client
                        .create_branch_from_tag(repository, &tag, &new, quiet)
                        .await?;
                }
            }
        }
        BranchCommand::Delete(delete_cmd) => {
            let repository = delete_cmd.repository.as_ref();
            if delete_cmd.all {
                client
                    .delete_all_branches(&delete_cmd.branch, &repos)
                    .await?;
            } else if let Some(repository) = repository {
                client.delete_branch(repository, &delete_cmd.branch).await?;
            }
        }
        BranchCommand::ChangeVersion(change_version_cmd) => {
            let repository = change_version_cmd.repository.clone();
            let message = change_version_cmd.message.clone();
            let (branch, old_version, new_version) = (
                change_version_cmd.branch.clone(),
                change_version_cmd.old_version.clone(),
                change_version_cmd.new_version.clone(),
            );
            if change_version_cmd.all {
                client
                    .change_all_release_version(
                        branch,
                        old_version,
                        new_version,
                        &repos[..],
                        message,
                        dry_run,
                    )
                    .await?;
            } else if let Some(repository) = &repository {
                client
                    .change_release_version(
                        repository.to_string(),
                        branch,
                        old_version,
                        new_version,
                        message,
                        dry_run,
                    )
                    .await?;
            }
        }
    }
    Ok(())
}

/// Process all Repo commands
async fn match_repo_cmds(
    client: &GithubClient,
    repos: Vec<String>,
    config: Config,
    cmd: &RepoCommand,
    fork_workaround: HashMap<String, String>,
) -> Result<(), GitError> {
    match cmd {
        RepoCommand::Sync(sync_cmd) => {
            let repository = sync_cmd.repository.as_ref();
            let recursive = sync_cmd.recursive;
            let branch = sync_cmd.branch.as_ref();
            let forks_with_workaround = config.repos.fork_workaround.clone().unwrap_or_default();

            if sync_cmd.all {
                if fork_workaround.is_empty() {
                    client.sync_all_forks(&repos[..], recursive).await?;
                } else {
                    let (task1, task2) = tokio::join!(
                        client.sync_all_forks_workaround(fork_workaround.clone()),
                        client.sync_all_forks(&repos[..], recursive),
                    );
                    task1?;
                    task2?;
                }
            } else if let Some(repository) = repository {
                // Process repositories using git
                if let Some(parent) = fork_workaround.get(repository) {
                    client.sync_with_upstream(repository, parent).await?;
                } else if forks_with_workaround.contains_key(repository) {
                    return Err(GitError::Other(format!(
                        "Repository '{repository}' has no configured parent. Use the '--with-fork-workaround' flag to enable syncing it."
                    )));
                } else if recursive {
                    client.sync_fork_recursive(repository).await?;
                } else {
                    client.sync_fork(repository, branch).await?;
                }
            } else {
                return Err(GitError::MissingRepositoryName);
            }
        }
        RepoCommand::Check(check_cmd) => {
            let repository = check_cmd.repository.as_ref();
            let (protected, license) = (check_cmd.protected, check_cmd.license);
            let old_branches = (check_cmd.old_branches, check_cmd.days_ago);
            let blacklist = config.misc.branch_blacklist.unwrap_or_default();
            let license_blacklist = config.misc.license_blacklist.unwrap_or_default();
            let filter = check_cmd.branch_filter.clone();
            let branch_filter = if let Some(filter) = filter {
                Some(Regex::new(&filter).map_err(|err| GitError::RegexError(Box::new(err)))?)
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

                if !client.is_tty {
                    println!("Repository,Branch,Date,License,Rules");
                }
                for res in &result {
                    let Checks {
                        branches,
                        rules,
                        license,
                        repo,
                    } = res.clone();
                    let check = res.clone();

                    let branches: Vec<Vec<String>> = branches
                        .iter()
                        .map(|(b, d)| vec![b.to_string(), d.to_string()])
                        .collect();
                    client.display_check_results(
                        vec!["Branch".to_string(), "Date".to_string()],
                        branches,
                        &rules,
                        license.as_ref(),
                        &repo,
                    );

                    client
                        .validate_check_results(
                            &repo,
                            check,
                            blacklist.clone(),
                            license_blacklist.clone(),
                        )
                        .await?;
                }
            } else if let Some(repository) = repository {
                let result = client
                    .check_repository(repository, blacklist.clone(), checks)
                    .await?;

                let Checks {
                    branches,
                    rules,
                    license,
                    repo,
                } = &result.clone();
                let branches = branches
                    .iter()
                    .map(|(b, d)| vec![b.to_string(), d.to_string()])
                    .collect();
                if !client.is_tty {
                    println!("Repository,Branch,Date,License,Rules");
                }
                client.display_check_results(
                    vec!["Branch".to_string(), "Date".to_string()],
                    branches,
                    rules,
                    license.as_ref(),
                    repo,
                );
                client
                    .validate_check_results(repository, result, blacklist, license_blacklist)
                    .await?;
            } else {
                return Err(GitError::MissingRepositoryName);
            }
        }
    }

    Ok(())
}

/// Process all PR commands
async fn match_pr_cmds(
    client: &GithubClient,
    repos: Vec<String>,
    _config: Config,
    cmd: &PRCommand,
) -> Result<(), GitError> {
    match cmd {
        PRCommand::Open(open_cmd) => {
            let repository = open_cmd.repository.clone().unwrap_or_default();
            let merge = open_cmd.merge;
            let opts = CreatePrOptions {
                url: repository.clone(),
                head: open_cmd.head.clone(),
                base: open_cmd.base.clone(),
                title: open_cmd.title.clone(),
                body: open_cmd.body.clone(),
                reviewers: open_cmd.reviewers.clone(),
                should_merge: merge,
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
                // Merging is handled within the create_all_prs method. It gets complicated
                // managing the SHA for the latest commit across multiple repositories otherwise
                let _pr_hashmap = client.create_all_prs(&opts, merge_opts, repos).await?;
            } else if !repository.len() > 0 {
                let Some((pr_number, sha)) = client.create_pr(&opts).await? else {
                    return Ok(());
                };
                if let Some(opts) = merge_opts.as_mut() {
                    opts.pr_number = pr_number;
                    if let Some(sha) = open_cmd.sha.clone() {
                        opts.sha = Some(sha);
                    } else {
                        opts.sha = Some(sha);
                    }
                    client.merge_pr(opts).await?;
                }
            } else {
                return Err(GitError::MissingRepositoryName);
            }
        }
    }
    Ok(())
}
/// Process all Release commands
async fn match_release_cmds(
    client: &GithubClient,
    repos: Vec<String>,
    _config: Config,
    cmd: &ReleaseCommand,
) -> Result<(), GitError> {
    match cmd {
        ReleaseCommand::Create(create_cmd) => {
            let repository = create_cmd.repository.as_ref();
            if create_cmd.all {
                client
                    .create_all_releases(
                        &create_cmd.current_release,
                        &create_cmd.previous_release,
                        create_cmd.release_name.as_deref(),
                        repos,
                    )
                    .await?;
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
    }

    Ok(())
}
/// Process all backup commands
async fn match_backup_cmds(
    client: &GithubClient,
    repos: Vec<String>,
    _config: Config,
    cmd: &BackupCommand,
    fork_workaround: HashMap<String, String>,
) -> Result<(), GitError> {
    match cmd {
        BackupCommand::Create(create_cmd) => {
            let repository = create_cmd.repository.as_ref();
            let passed_path = create_cmd.path.as_ref();
            let current_dir;
            let dest = create_cmd.destination;
            let path = if let Some(p) = passed_path {
                p
            } else {
                current_dir = env::current_dir()?;
                current_dir.as_path()
            };
            let bucket = create_cmd.bucket.as_ref();

            if create_cmd.all {
                let mut repos = repos;
                if !fork_workaround.is_empty() {
                    repos.extend(fork_workaround.keys().cloned());
                }
                let successful = client.backup_all_repos(&repos[..], path).await?;
                if dest == BackupDestination::S3 {
                    if let Some(bucket) = bucket {
                        client.backup_all_to_s3(successful, bucket).await?;
                    } else {
                        return Err(GitError::Other(
                            "No bucket provided for aws backup".to_string(),
                        ));
                    }
                    return Ok(());
                }
            } else if let Some(repository) = repository {
                let repo_dist = client.backup_repo(repository.to_string(), path).await?;
                if dest == BackupDestination::S3 {
                    if let Some(bucket) = bucket {
                        client.backup_to_s3(&repo_dist, bucket).await?;
                    } else {
                        return Err(GitError::Other(
                            "No bucket provided for aws backup".to_string(),
                        ));
                    }
                    return Ok(());
                }
            }
        }
        BackupCommand::Clean(clean_cmd) => {
            let path = clean_cmd.directory.clone();
            client.prune_backup(&path, None).await?;
        }
    }
    Ok(())
}

/// Helper to write man pages
fn write_man(cmd: &mut clap::Command, out_dir: &Path, name: &str) {
    let man = Man::new(cmd.clone());
    let mut file = File::create(out_dir.join(name)).unwrap();
    man.render(&mut file).unwrap();
}

/// Generate man pages for all subcommands. Otherwise, we only get a manpage for the root command.
fn generate_man_pages(mut cmd: clap::Command, out_dir: &PathBuf, parent: Option<String>) {
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
        generate_man_pages(subcommand.clone(), out_dir, Some(sub_name));
    }
}
