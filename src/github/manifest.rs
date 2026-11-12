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

//! Execute validated component-manifest reference plans.

use crate::error::GitError;
use crate::github::client::GithubClient;
use crate::manifest::ManifestRefPlan;
use futures::{StreamExt, stream::FuturesUnordered};
use std::future::Future;

/// Create all branches in a validated component-manifest plan.
pub async fn create_manifest_branches(
    client: &GithubClient,
    plan: Vec<ManifestRefPlan>,
    quiet: bool,
    dry_run: bool,
) -> Result<(), GitError> {
    if dry_run {
        print_plan("branch", &plan);
        return Ok(());
    }

    let count = plan.len();
    let futures = FuturesUnordered::new();
    for operation in plan {
        futures.push(async move {
            let result = client
                .create_branch_from_sha(
                    &operation.repository,
                    &operation.sha,
                    &operation.target_ref,
                    quiet,
                )
                .await;
            (operation, result)
        });
    }

    collect_results("branch", count, futures).await
}

/// Create all tags in a validated component-manifest plan.
pub async fn create_manifest_tags(
    client: &GithubClient,
    plan: Vec<ManifestRefPlan>,
    dry_run: bool,
) -> Result<(), GitError> {
    if dry_run {
        print_plan("tag", &plan);
        return Ok(());
    }

    let count = plan.len();
    let futures = FuturesUnordered::new();
    for operation in plan {
        futures.push(async move {
            let result = client
                .create_tag_from_sha(&operation.repository, &operation.target_ref, &operation.sha)
                .await;
            (operation, result)
        });
    }

    collect_results("tag", count, futures).await
}

/// Text replacement settings applied to every branch in a manifest plan.
#[derive(Clone, Debug)]
pub struct ManifestModifyOptions {
    /// The text to replace.
    pub old_text: String,
    /// The text to replace it with.
    pub new_text: String,
    /// Optional commit message that overrides the automatic one.
    pub message: Option<String>,
    /// Prepare the changes without pushing them.
    pub dry_run: bool,
    /// Whether the replaced text is a version (enables the odp-bigtop special cases).
    pub is_version: bool,
    /// Reduce output.
    pub quiet: bool,
}

/// Replace text in every branch of a validated component-manifest plan.
///
/// Each branch is cloned, edited, committed and pushed independently, so a repository that
/// appears with several versions (for example livy) is handled in the same run.
///
/// During a dry run a branch that does not exist yet, such as one that `branch create` has not
/// made, is reported and skipped rather than treated as an error. That lets a full create,
/// modify, tag dry run succeed before anything has been written.
pub async fn modify_manifest_branches(
    client: &GithubClient,
    plan: Vec<ManifestRefPlan>,
    options: ManifestModifyOptions,
) -> Result<(), GitError> {
    if options.dry_run {
        print_modify_plan(&plan, &options);
    }

    let mut futures = FuturesUnordered::new();
    for operation in plan {
        let options = options.clone();
        futures.push(async move {
            let result = modify_one(client, &operation, options).await;
            (operation, result)
        });
    }

    let mut errors = Vec::new();
    while let Some((operation, result)) = futures.next().await {
        if let Err(error) = result {
            eprintln!(
                "Failed to modify branch '{}' for {} ({}): {error}",
                operation.target_ref, operation.component, operation.repository
            );
            errors.push((
                format!("{} ({})", operation.component, operation.target_ref),
                error,
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        // Every branch in a manifest is part of the same release, so any failed edit must fail
        // the run. `ErrorWithRepoInfo` exits 0 when fewer errors than repositories were
        // reported, so report the error count as the repository count to always exit non-zero.
        let failed = errors.len();
        Err(GitError::ErrorWithRepoInfo(
            Box::new(GitError::MultipleErrors(errors)),
            failed,
        ))
    }
}

async fn modify_one(
    client: &GithubClient,
    operation: &ManifestRefPlan,
    options: ManifestModifyOptions,
) -> Result<(), GitError> {
    if options.dry_run
        && !branch_exists(client, &operation.repository, &operation.target_ref).await?
    {
        println!(
            "Branch '{}' does not exist in {} yet; skipping the dry-run edit for {}",
            operation.target_ref, operation.repository, operation.component
        );
        return Ok(());
    }

    client
        .modify_branch(
            operation.repository.clone(),
            operation.target_ref.clone(),
            options.old_text,
            options.new_text,
            options.message,
            options.dry_run,
            options.is_version,
            options.quiet,
        )
        .await
}

/// Whether a branch exists, asking GitHub directly instead of using the on-disk cache so a
/// branch that was just created is seen.
async fn branch_exists(
    client: &GithubClient,
    repository: &str,
    branch: &str,
) -> Result<bool, GitError> {
    match client.get_branch_sha(repository, branch).await {
        Ok(_) => Ok(true),
        Err(GitError::NoSuchBranch(_)) => Ok(false),
        Err(GitError::GithubApiError(octocrab::Error::GitHub { source, .. }))
            if source.status_code == http::StatusCode::NOT_FOUND =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

async fn collect_results<F>(
    kind: &str,
    count: usize,
    mut futures: FuturesUnordered<F>,
) -> Result<(), GitError>
where
    F: Future<Output = (ManifestRefPlan, Result<(), GitError>)>,
{
    let mut errors = Vec::new();
    while let Some((operation, result)) = futures.next().await {
        match result {
            Ok(()) => println!(
                "Successfully created {kind} '{}' for {} ({})",
                operation.target_ref, operation.component, operation.repository
            ),
            Err(error) => {
                eprintln!(
                    "Failed to create {kind} '{}' for {} ({}): {error}",
                    operation.target_ref, operation.component, operation.repository
                );
                errors.push((operation.component, error));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(GitError::ErrorWithRepoInfo(
            Box::new(GitError::MultipleErrors(errors)),
            count,
        ))
    }
}

fn print_modify_plan(plan: &[ManifestRefPlan], options: &ManifestModifyOptions) {
    println!(
        "DRY RUN: '{}' would be replaced with '{}' in {} branch(es)",
        options.old_text,
        options.new_text,
        plan.len()
    );
    for operation in plan {
        println!(
            "{}\t{}\t{}",
            operation.component, operation.repository, operation.target_ref
        );
    }
}

fn print_plan(kind: &str, plan: &[ManifestRefPlan]) {
    println!(
        "DRY RUN: {} {kind} reference(s) would be created",
        plan.len()
    );
    for operation in plan {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            operation.component,
            operation.repository,
            operation.source_branch,
            operation.target_ref,
            operation.sha
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_plan_does_not_require_a_client() {
        let plan = [ManifestRefPlan {
            component: "hadoop".to_string(),
            repository: "https://github.com/acceldata-io/hadoop".to_string(),
            source_branch: "nightly/ODP-3.3.6.5".to_string(),
            target_ref: "rel/ODP-3.3.6.6-1".to_string(),
            sha: "68e511577e6e77e3f47427d05643d076bfe896ca".to_string(),
        }];
        print_plan("branch", &plan);
    }
}
