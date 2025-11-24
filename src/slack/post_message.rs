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

use reqwest::Client;
use std::collections::HashMap;

/// Post data to a Slack channel using the provided webhook URL.
/// This can be used to send notifications or messages to update a slack channel about
#[cfg(feature = "slack")]
pub async fn send_slack_message(
    webhook_url: String,
    data: HashMap<String, String>,
) -> Result<(), reqwest::Error> {
    let client = Client::new();
    let res = client
        .post(webhook_url.clone())
        .json(&data)
        .send()
        .await?;
    if !res.status().is_success() {
        eprintln!(
            "Failed to post message to channel, with url {}: {}",
            webhook_url,
            res.status()
        );
    }
    Ok(())
}
