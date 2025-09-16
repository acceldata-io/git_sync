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

#![warn(clippy::all, clippy::pedantic, missing_docs)]

//! Manage Github repositories from the command line.
//! You can perform an action on one repository, or on all repositories that are configured in the
//! configuration file.
//!
//! This tool is still in development so some features may not work as expected
//!
//! You can do the following:
//! 1. Sync a forked repository with its upstream parent repository.
//! 2. Sync tags from its parent repository
//! 3. Manage branches (create, delete)
//! 4. Manage tags (create, delete)
//! 5. Create releases with automatically generated release notes
//! 6. Create and merge pull requests
//! 7. Run various sanity checks on repositories
//! 8. Create a PR and automatically merge it, if possible
//! 9. Backup a repository, including to S3 if enabled (In progress)
//! 10. Collect metadata about your repositories (TODO)
//! 11. Send notifications to slack if the slack feature is enabled
//! 11. Do all of the above for one, or all configured repositories
//!
//! Some examples of how to use this tool:
//! ```shell
//! git_sync tag create --tag v1.0.2 --branch my_dev_branch --repo https://github.com/org/my_repository
//! git_sync tag create --tag v1.0.0 --branch my_common_dev_branch --all --slack # will use all repos in your config file and enable slack notifications
//! ```
//! When running a command with --all, you can specify which category of repositories to run the
//! command on by specifying the `--repository` argument (valid options are 'fork', 'private',
//! 'public', or 'all').
//!
//! You can generate manpages and shell completions using the generate command:
//! ```shell
//! git_sync generate --kind man
//! git_sync generate --kind bash # or fish, or zsh
//! ```
//!
//! When building this tool, you should use
//! ```shell
//! cargo auditable build --release
//! ```
//!
//! This can be installed by running
//! ```shell
//! cargo install cargo-auditable
//! ```
//!
//! This way the binary will be built with information about the build dependencies embedded into
//! the binary so that it can be scanned with tools like cargo-audit or Trivy.
mod cli;
mod config;
mod error;
mod github;
mod init;
mod slack;
mod utils;

use cli::parse_args;
use config::Config;
use error::GitError;
use github::match_args;
use rustls::crypto::aws_lc_rs;
#[tokio::main]
async fn main() -> Result<(), GitError> {
    // This needs to be here to make sure we're using aws-lc-rs.
    // aws-lc-rs is actively being developed.
    // One or two crates default to ring, so we override it here.
    let provider = aws_lc_rs::default_provider().install_default();
    if provider.is_err() {
        return Err(GitError::Other(
            "Failed to install AWS-LC-RS as default TLS provider".to_string(),
        ));
    }
    let args = parse_args();

    let result: Result<(), GitError> = {
        let config = Config::new(args.file.as_ref())?;
        match_args::match_arguments(&args, config).await
    };
    // Get nice error messages, with simple suggestions instead of huge structs
    if let Err(e) = result {
        if e.is_broken_pipe() {
            std::process::exit(0)
        }
        let error = e.to_user_error();
        eprintln!("{error}");
        std::process::exit(1);
    }

    Ok(())
}
