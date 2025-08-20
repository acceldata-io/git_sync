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


/// Handles the result of an API call using octocrab and maps them
/// to a GitError. This helps reduce a lot of unnecessary boilerplate.
/// This is used for api calls that don't already have a high level
/// function provided by Octocrab.
///
/// #arguments
/// * `$response` -- The result to match.
/// * `$context` -- A string describing the context of what you're trying to handle, for logging.
/// * `$ok` -- The closure to run if the result is Ok.
#[macro_export]
macro_rules! handle_api_response {
    (
        $response:expr,
        $context:expr,
        $ok:expr,
    ) => {
        match $response {
            Ok(body) => $ok(body),
            Err(err) => {
                let (status, message) = get_http_status(&err);
                match (status, message) {
                    (Some(code), Some(message)) => eprintln!(
                        "{}: HTTP {:?} - {:?}",
                        $context, code, message
                    ),    
                    (Some(code), None) => eprintln!(
                        "{}: HTTP {:?}",
                        $context, code
                    ),
                    _ => eprintln!(
                        "{}: {:?}",
                        $context, err
                    ),
                }
                return Err(GitError::GithubApiError(err));
            }
        }
    };
}

