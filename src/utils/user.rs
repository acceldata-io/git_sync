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
use std::process::Command;

/// Hold information about the git user
#[allow(dead_code)]
#[derive(Debug)]
pub struct UserDetails {
    /// Name of the user
    pub name: String,
    /// Email of the user
    pub email: String,
}

impl UserDetails {
    /// Create a new UserDetails instance
    pub fn new(name: String, email: String) -> Self {
        UserDetails { name, email }
    }
    /// Try to load user information from the environment through git
    pub fn new_from_git() -> Result<Self, std::io::Error> {
        let name_output = Command::new("git")
            .arg("config")
            .arg("get")
            .arg("user.name")
            .output()?;
        let email_output = Command::new("git")
            .arg("config")
            .arg("get")
            .arg("user.email")
            .output()?;

        match (name_output.status.success(), email_output.status.success()) {
            (true, true) => {
                let name = String::from_utf8_lossy(&name_output.stdout)
                    .trim()
                    .to_string();
                let email = String::from_utf8_lossy(&email_output.stdout)
                    .trim()
                    .to_string();
                Ok(UserDetails { name, email })
            }
            (false, _) => Err(std::io::Error::other("Failed to get git user name")),
            (_, false) => Err(std::io::Error::other("Failed to get git email address")),
        }
    }
}
