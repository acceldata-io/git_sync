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

use clap::ValueEnum;

/// Options for PR creation.
#[derive(Clone)]
pub struct CreatePrOptions {
    /// Url of the repository where the PR should be created.
    pub url: String,
    /// The branch where changes are implemented.
    pub head: String,
    /// The branch where changes should be merged into.
    pub base: String,
    /// Title of the PR.
    pub title: String,
    /// Body of the PR.
    pub body: Option<String>,
    /// A list of reviewers to request a review from.
    pub reviewers: Option<Vec<String>>,
    /// Whether this should be merged. If it isn't going to be, we can skip grabbing the SHA and PR
    /// number for this.
    pub should_merge: bool,
}

/// Options for merging a PR.
#[derive(Clone)]
pub struct MergePrOptions {
    /// Url of the repository where the PR exists.
    pub url: String,
    /// PR number to merge.
    pub pr_number: u64,
    /// The method to use for merging the PR. Defaults to 'merge' with the GitHub API.
    pub method: MergeMethod,
    /// Title for the automatic commit message.
    pub title: Option<String>,
    /// Extra detail to append to automatic commit message.
    pub message: Option<String>,
    /// SHA that pull request head must match to allow merge.  
    pub sha: Option<String>,
}

#[derive(ValueEnum, Copy, Clone, Debug, Default)]
/// Github sets the default merge method to 'merge'
pub enum MergeMethod {
    #[default]
    Merge,
    Squash,
    Rebase,
}

impl From<MergeMethod> for octocrab::params::pulls::MergeMethod {
    fn from(method: MergeMethod) -> Self {
        match method {
            MergeMethod::Merge => octocrab::params::pulls::MergeMethod::Merge,
            MergeMethod::Squash => octocrab::params::pulls::MergeMethod::Squash,
            MergeMethod::Rebase => octocrab::params::pulls::MergeMethod::Rebase,
        }
    }
}
