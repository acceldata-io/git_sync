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
/// # Parameters
/// * `$response` -- The result to match.
/// * `$context` -- A string describing the context of what you're trying to handle, for logging.
/// * `$ok` -- The closure to run if the result is Ok.
///
/// # Returns
/// Returns the result of `$ok` if successful, or a `GitError` if the API call fails.
/// You can use '?' to propagate the error to your function.
///
/// ```rust
/// async fn sync_fork(owner: &str, repo: &str, parent_owner: &str, parent_repo: &str, main_branch: &str) -> Result<(), GitError> {
///     let octocrab = octocrab::instance();
///     let body = serde_json::json!({"branch": main_branch.to_string()});
///     let response: Result<serde_json::Value, octocrab::Error> = octocrab
///         .post(format!("/repos/{owner}/{repo}/merge-upstream"), Some(&body))
///         .await;
///
///     handle_api_response!(
///         response,
///         format!("Unable to sync {owner}/{repo} with {parent_owner}/{parent_repo}"),
///         |_| {
///             println!("Successfully synced {owner}/{repo} with {parent_owner}/{parent_repo}");
///             Ok::<(), GitError>(())
///         })?;
///        Ok(())
///    }
/// ```
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
                let (status, message) = $crate::utils::repo::get_http_status(&err);
                match (status, message) {
                    (Some(code), Some(message)) => {
                        eprintln!("{}: HTTP {:?} - {:?}", $context, code, message)
                    }
                    (Some(code), None) => eprintln!("{}: HTTP {:?}", $context, code),
                    _ => eprintln!("{}: {:?}", $context, err),
                }
                return Err(GitError::GithubApiError(err));
            }
        }
    };
}

/// Handles reducing some of the boiler plate for defining these unordered futures.
///
/// # Parameters
/// `$iter`` is some iterable collection
/// `|$($arg:ident),*| $future:expr`: Closure-like syntax where you specify the variable(s)
/// `($($pat:pat),*) $body:block`: Pattern(s) to destructure the resulting tuple from the completed future
/// and a block of code to process each result.
///
/// # Example
/// ```rust
/// handle_futures_unordered!(
///     vec.iter().map(|x| (x.clone(),)), |x| async move {
///         (x, async_function(x).await)
///     },
///     (x, result) {
///         match result {
///             Ok(val) => println!("Success: {x}: {val}"),
///             Err(e) => eprintln!("Error: {x}: {e}"),
///         }
///     }
/// );
/// pub async fn sync_all_forks(&self, repos: Vec<String>) -> Result<(), GitError> {
///     handle_futures_unordered!(
///         repos.iter().map(|repo| (repo.clone(),)),
///         |repo| self.sync_fork(&repo),
///         (repo, result) {
///             match result {
///                 Ok(_) => println!("Successfully synced fork: {}", repo),
///                 Err(e) => eprintln!("Error syncing fork {}: {}", repo, e),
///             }
///         }
///     );
/// }
///
/// ```
/// # Notes
/// - The macro requires the `futures` crate.
/// - The async block must return a tuple matching the destructuring pattern used in the `while let Some(...)`.
/// - All generated futures must have the same return type.
///
/// # See also
/// - [`FuturesUnordered`](https://docs.rs/futures/latest/futures/stream/struct.FuturesUnordered.html)
#[macro_export]
macro_rules! handle_futures_unordered {
    (
        $iter:expr, |$($arg:ident),*| $future:expr, ($($pattern:pat),*) $body:expr
    ) => {{
        let mut futures = ::futures::stream::FuturesUnordered::new();
        for ($($arg),*) in $iter {
            futures.push(async move {$future.await});
        }
        while let Some(($($pattern),*)) = futures.next().await {
            $body
        }

    }};
}

/// A macro to easily retry asynchronous operations with exponential backoff and optional error filtering.
///
/// # Overview
/// The `async_retry!` macro wraps an asynchronous block and automatically retries it using an
/// exponential backoff schedule. It allows you to specify the initial delay, maximum delay,
/// number of retries, a custom error predicate, and the operation body.
///
/// Internally, it uses [`tokio_retry::RetryIf`] with [`tokio_retry::strategy::ExponentialBackoff`],
/// applying jitter to the delays.
///
/// # Parameters
/// - `ms = $ms`: The base delay in milliseconds for the exponential backoff.
/// - `timeout = $timeout`: The maximum delay between retries, in milliseconds.
/// - `retries = $retries`: The maximum number of retry attempts.
/// - `error_predicate = $pred`: A function or closure that takes a reference to an error and returns
///   `true` if the error is retryable, or `false` otherwise.
/// - `body = $body`: The asynchronous block to execute and retry on failure.
///
/// # Example
/// ```rust
/// use tokio_retry::strategy::ExponentialBackoff;
/// use tokio_retry::RetryIf;
///
/// async fn unreliable_network_call() -> Result<(), std::io::Error> {
///     // ... implementation ...
///     Ok(())
/// }
///
/// let result: Result<(), std::io::Error> = async_retry!(
///     ms = 100,
///     timeout = 2000,
///     retries = 5,
///     error_predicate = |e: &std::io::Error| matches!(e.kind(), std::io::ErrorKind::TimedOut),
///     body = {
///         unreliable_network_call().await
///     },
/// );
/// ```
///
/// # Notes
/// - The macro requires the `tokio_retry` and `tokio` crates.
/// - The `body` block must be `async`.
/// - The retry strategy applies exponential backoff with jitter and is capped by the specified timeout.
///
/// # See Also
/// - [`tokio_retry::RetryIf`](https://docs.rs/tokio-retry/latest/tokio_retry/struct.RetryIf.html)
/// - [`tokio_retry::strategy::ExponentialBackoff`](https://docs.rs/tokio-retry/latest/tokio_retry/strategy/struct.ExponentialBackoff.html)
#[macro_export]
macro_rules! async_retry {
    (
        ms = $ms: expr,
        timeout = $timeout: expr,
        retries = $retries: expr,
        error_predicate = $pred: expr,
        body = $body: block,
    ) => {{
        let retry_strategy = tokio_retry::strategy::ExponentialBackoff::from_millis($ms)
            .max_delay(tokio::time::Duration::from_millis($timeout))
            .map(tokio_retry::strategy::jitter)
            .take($retries);

        tokio_retry::RetryIf::spawn(retry_strategy, || async { $body }, $pred).await
    }};
}
