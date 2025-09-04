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
mod cli;
mod config;
mod error;
mod github;
mod init;
mod utils;

use cli::*;
use config::Config;
use error::GitError;
use github::parse;
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
        let config = Config::new(&args.file)?;
        parse::match_arguments(&args, config).await
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
