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
use crate::async_retry;
use crate::error::{GitError, is_retryable};
use crate::github::client::GithubClient;
use crate::utils::repo::{
    BranchProtectionRule, Checks, LicenseInfo, RepoChecks, get_repo_info_from_url,
};
use crate::utils::tables::Table;
use chrono::{DateTime, Utc};
use futures::{StreamExt, stream::FuturesUnordered};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

impl GithubClient {
    /// Get branches that are older than a certain number of days
    /// A blacklist should be passed to ignore certain branches, since some of them are static and
    /// will never change, nor should they be deleted.
    #[allow(clippy::too_many_lines)]
    pub async fn check_repository(
        &self,
        url: &str,
        blacklist: HashSet<String>,
        checks: &RepoChecks,
    ) -> Result<Checks, GitError> {
        let info = get_repo_info_from_url(url)?;
        let (owner, repo) = (info.owner, info.repo_name);
        let octocrab = self.octocrab.clone();
        let (get_branches, days_ago) = checks.old_branches;
        let get_protection = checks.protected;
        let get_license = checks.license;
        let branch_filter = checks.branch_filter.clone();

        let mut protection_rules: Vec<BranchProtectionRule> = Vec::new();
        let mut license: Option<LicenseInfo>;

        // Use a graphql query to drastically reduce the number of api calls we need to make.
        // This lets us get the repo name and latest commit date in one call, instead of two.
        // This makes a huge difference for very large repositories with many branches.
        let query = r#"
            query($owner: String!, $repo: String!, $after: String, $getLicenseInfo: Boolean!, $getBranches: Boolean!, $getProtection: Boolean!) {
                repository(owner: $owner, name: $repo) {
                    licenseInfo @include(if: $getLicenseInfo) {
                        name
                        spdxId
                        url
                    }
                    branchProtectionRules(first: 100) @include(if: $getProtection){
                        nodes {
                            pattern
                            isAdminEnforced
                            requiresApprovingReviews
                            requiredApprovingReviewCount
                            requiresStatusChecks
                            requiresStrictStatusChecks
                            requiresConversationResolution
                            restrictsPushes
                            restrictsReviewDismissals
                        }
                    }

                    refs(refPrefix: "refs/heads/", first: 100, after: $after) @include(if: $getBranches) {
                        nodes {
                            name
                            target {
                                ... on Commit {
                                    committedDate
                                }
                            }
                        }
                        pageInfo {
                            hasNextPage
                            endCursor
                        }
                    }
                }
            }
        "#;

        let mut branches = HashMap::<String, DateTime<Utc>>::new();

        // Needed for the graphql query
        let mut after: Option<String> = None;

        loop {
            // Acquire a lock on the semaphore
            let permit = self.semaphore.clone().acquire_owned().await?;

            // Paginated results, so we have to loop over until there aren't any more pages left
            let payload = serde_json::json!({
                "query": query,
                "variables": {
                    "owner": owner,
                    "repo": repo,
                    "after": after,
                    "getBranches": get_branches,
                    "getProtection": get_protection,
                    "getLicenseInfo": get_license,
                }
            });
            let response: serde_json::Value = async_retry!(
                ms = 100,
                timeout = 5000,
                retries = 3,
                error_predicate = |e: &octocrab::Error| is_retryable(e),
                body = { octocrab.graphql(&payload).await },
            )?;
            drop(permit);
            let refs = &response["data"]["repository"]["refs"];
            license = response["data"]["repository"]["licenseInfo"]
                .as_object()
                .and_then(|license| {
                    serde_json::from_value::<LicenseInfo>(serde_json::Value::Object(
                        license.clone(),
                    ))
                    .ok()
                });

            if let Some(nodes) = refs["nodes"].as_array() {
                for branch in nodes {
                    if let Some(name) = branch.get("name").and_then(|v| v.as_str()) {
                        if let Some(re) = &branch_filter
                            && !re.is_match(name)?
                        {
                            continue;
                        }

                        let date_str = branch
                            .pointer("/target/committedDate")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let date = date_str.parse::<DateTime<Utc>>();
                        match date {
                            Ok(d) => {
                                branches.insert(name.to_string(), d);
                            }
                            Err(e) => {
                                eprintln!(
                                    "Failed to parse date for {owner}/{repo} branch {name}: {e}"
                                );
                            }
                        }
                    }
                }
            }
            if let Some(protection) =
                response["data"]["repository"]["branchProtectionRules"]["nodes"].as_array()
            {
                for rule in protection {
                    let parsed_rule: Option<BranchProtectionRule> =
                        serde_json::from_value(rule.clone()).ok();
                    if let Some(r) = parsed_rule {
                        protection_rules.push(r);
                    }
                }
            }

            let page_info = &refs["pageInfo"];
            let has_next_page = page_info["hasNextPage"].as_bool().unwrap_or(false);
            after = page_info["endCursor"]
                .as_str()
                .map(std::string::ToString::to_string);

            if !has_next_page {
                break;
            }
        }
        let now = Utc::now();

        let mut old_branches: Vec<_> = branches
            .into_iter()
            .filter(|(_, age)| (now - age).num_days() >= days_ago)
            .filter(|(branch, _)| !blacklist.contains(branch))
            .collect();
        old_branches.sort_by_key(|(_, age)| *age);

        let old_branches: Vec<(String, String)> = old_branches
            .into_iter()
            .map(|(branch, age)| {
                let formatted = age.format("%Y-%m-%d").to_string();
                (branch.clone(), formatted.clone())
            })
            .collect();

        Ok(Checks {
            branches: old_branches,
            rules: protection_rules,
            license,
            repo: format!("{owner}/{repo}"),
        })
    }
    /// Display the results of the various checks into a nice table
    pub fn display_check_results(
        &self,
        header: Vec<String>,
        rows: Vec<Vec<String>>,
        rules: &[BranchProtectionRule],
        license: Option<&LicenseInfo>,
        repo: &str,
    ) {
        if self.is_tty {
            println!("TTY?");
            let table = Table::builder(tabled::settings::style::Style::ascii())
                .title(format!("Stale Branches for {repo}"))
                .header(header)
                .rows(rows)
                .centre(false)
                .align(tabled::settings::Alignment::center())
                .build();
            println!("{table}");
            if !rules.is_empty() {
                for rule in rules {
                    println!("{rule}");
                }
            }
            if let Some(license) = &license
                && let Some(name) = &license.name
            {
                println!("License: {name}");
            }
        } else {
            let mut output = repo.to_string();
            let license_name = if let Some(license) = &license
                && let Some(name) = &license.name
            {
                format!(",{name}")
            } else {
                String::new()
            };
            for v in rows {
                let line = v.join(",");
                let _ = write!(output, "{line}");
                let line = format!("{repo},{line},{license_name}");
                println!("{line}");
            }
            if let Some(license) = &license
                && let Some(name) = &license.name
            {
                let _ = write!(output, ",{name}");
            }
        }
    }
    /// Check the results and send any errors to slack. None of our repositories currently have any
    /// rules
    pub async fn validate_check_results(
        &self,
        repository: &str,
        check: Checks,
        branch_blacklist: HashSet<String>,
        license_blacklist: HashSet<String>,
    ) -> Result<(), GitError> {
        if let Some(l) = &check.license
            && let Some(spdx) = &l.spdx_id
            && license_blacklist.contains(spdx)
        {
            self.append_slack_error(format!("Blacklisted license {spdx} used in {repository}"))
                .await;
        }

        if !check.branches.is_empty() {
            let branches: Vec<(String, String)> = check
                .branches
                .into_iter()
                .filter(|(branch, _)| !branch_blacklist.contains(branch))
                .collect();
            let num_branches = branches.len();
            if num_branches > 0 {
                self.append_slack_error(format!(
                    "Repository {repository} has {num_branches} stale branches"
                ))
                .await;
            }
        }

        Ok(())
    }
    /// Run the selected checks against all configured repositories
    pub async fn check_all_repositories(
        &self,
        repositories: Vec<String>,
        checks: &RepoChecks,
        blacklist: &HashSet<String>,
    ) -> Result<Vec<Checks>, GitError> {
        let mut futures = FuturesUnordered::new();
        for repo in repositories {
            futures.push(async move {
                let response = self
                    .check_repository(&repo, blacklist.clone(), checks)
                    .await;
                (response, repo)
            });
        }

        let mut results: Vec<Checks> = Vec::new();
        let mut errors: Vec<(String, GitError)> = Vec::new();
        while let Some((result, repo)) = futures.next().await {
            match result {
                Ok(r) => results.push(r),
                Err(e) => errors.push((repo, e)),
            }
        }
        if errors.is_empty() {
            Ok(results)
        } else {
            Err(GitError::MultipleErrors(errors))
        }
    }
}
