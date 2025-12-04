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
use crate::error::GitError;
use fancy_regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::RwLock;

// Thread-safe cache for compiled regexes
// Wrapping it in a OnceLock ensures that we only initialize the RwLock<HashMap...> once
static REGEX_CACHE: OnceLock<RwLock<HashMap<String, Arc<Regex>>>> = OnceLock::new();

fn cache() -> &'static RwLock<HashMap<String, Arc<Regex>>> {
    REGEX_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Gets or compiles a regex pattern, saving it to a Hashmap cache for future use.
/// If something is wrong with the `RwLock`, this function will panic since that
/// means we have no way to recover.
pub fn get_or_compile<T>(pattern: T) -> Result<Arc<Regex>, Box<fancy_regex::Error>>
where
    T: AsRef<str>,
{
    let read_lock = cache().read().expect("Regex cache mutex poisoned");
    if let Some(regex) = read_lock.get(pattern.as_ref()) {
        return Ok(Arc::clone(regex));
    }
    // Intentionally drop the read lock before acquiring the write lock
    drop(read_lock);

    let mut write_lock = cache().write().expect("Regex cache mutex poisoned");
    // If some other thread has already compiled this regex between when we got the read_lock and
    // the write_lock, return the result instead of recompiling it. This should only be able to
    // happen on the first couple of calls
    if let Some(regex) = write_lock.get(pattern.as_ref()) {
        return Ok(Arc::clone(regex));
    }
    let regex_result = Regex::new(pattern.as_ref());
    match regex_result {
        Ok(re) => {
            let regex = Arc::new(re);
            write_lock.insert(pattern.as_ref().to_string(), Arc::clone(&regex));
            Ok(regex)
        }
        Err(e) => Err(std::boxed::Box::new(e)),
    }
}

/// Generic function to filter some collection of a particular regex filter
/// The regex gets compiled only the first time this function is called,
/// so it's relatively cheap to call multiple times with the same regex.
pub fn filter_ref<'a, I, A, S, T>(collection: I, regex: S) -> Result<T, GitError>
where
    I: IntoIterator<Item = &'a A>,
    A: AsRef<str> + 'a + ?Sized,
    S: AsRef<str> + std::fmt::Display,
    T: FromIterator<String>,
{
    // There isn't much reason to actually call this with an empty regex,
    // but just in case simply return the original collection as owned strings.
    if regex.as_ref().is_empty() {
        return Ok(collection
            .into_iter()
            .map(|item| item.as_ref().to_string())
            .collect());
    }

    let filter = match get_or_compile(regex.as_ref()) {
        Ok(re) => re,
        Err(e) => {
            return Err(GitError::RegexError(e));
        }
    };

    Ok(collection
        .into_iter()
        .filter_map(|item| {
            let item_str = item.as_ref();
            match filter.is_match(item_str) {
                Ok(true) => Some(item_str.to_string()),
                Ok(false) | Err(_) => None,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    #[test]
    fn test_filter_vec() {
        let v = vec!["Alice", "Bob", "Carey", "Zelda"];
        let result: Vec<String> = filter_ref(&v, "^A|^Z").expect("Failed to compile test regex");

        assert!(result.contains(&String::from("Alice")));
        assert!(result.contains(&String::from("Zelda")));
        assert_eq!(result.len(), 2);
    }
    #[test]
    fn test_filter_hashset() {
        let mut hs: HashSet<String> = HashSet::new();
        hs.insert("Alice".to_string());
        hs.insert("Amy".to_string());
        hs.insert("Bob".to_string());
        hs.insert("Carey".to_string());
        hs.insert("Zelda".to_string());
        let result: HashSet<String> =
            filter_ref(&hs, "^C|^Z").expect("Failed to compile test regex");

        assert!(result.contains("Carey"));
        assert!(!result.contains("Amy"));
        assert!(result.contains("Zelda"));
        assert_eq!(result.len(), 2);
    }
}
