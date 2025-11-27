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
use fancy_regex::Regex;
use std::sync::OnceLock;

static FILTER_REGEX: OnceLock<Regex> = OnceLock::new();
static REGEX_STRING: OnceLock<String> = OnceLock::new();

/// Generic function to filter some collection of a particular regex filter
/// The regex gets compiled only the first time this function is called,
/// so it's relatively cheap to call multiple times with the same regex.
pub fn filter_ref<'a, I, A, S, T>(collection: I, regex: S) -> T
where
    I: IntoIterator<Item = &'a A>,
    A: AsRef<str> + 'a + ?Sized,
    S: AsRef<str>,
    T: FromIterator<String>,
{
    // There isn't much reason to actually call this with an empty regex,
    // but just in case simply return the original collection as strings.
    if regex.as_ref().is_empty() {
        return collection
            .into_iter()
            .map(|item| item.as_ref().to_string())
            .collect();
    }

    REGEX_STRING.get_or_init(|| regex.as_ref().to_string());
    assert!(
        REGEX_STRING.get().unwrap().as_str() == regex.as_ref(),
        "filter_ref must be called with the same regex each time"
    );

    let filter = FILTER_REGEX.get_or_init(|| {
        let msg = format!("Invalid regex: {:?}", regex.as_ref());
        Regex::new(regex.as_ref()).expect(&msg)
    });

    collection
        .into_iter()
        .filter_map(|item| {
            let item_str = item.as_ref();
            match filter.is_match(item_str) {
                Ok(true) => Some(item_str.to_string()),
                Ok(false) | Err(_) => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    #[test]
    fn test_filter_vec() {
        let v = vec!["Alice", "Bob", "Carey", "Zelda"];
        let result: Vec<String> = filter_ref(&v, "^A|^Z");

        assert!(result.contains(&String::from("Alice")));
        assert!(result.contains(&String::from("Zelda")));
        assert_eq!(result.len(), 2);
    }
    #[test]
    fn test_string_hashset() {
        let mut hs: HashSet<String> = HashSet::new();
        hs.insert("Alice".to_string());
        hs.insert("Amy".to_string());
        hs.insert("Bob".to_string());
        hs.insert("Carey".to_string());
        hs.insert("Zelda".to_string());
        let result: HashSet<String> = filter_ref(&hs, "^A|^Z");

        assert!(result.contains("Alice"));
        assert!(result.contains("Amy"));
        assert!(result.contains("Zelda"));
        assert_eq!(result.len(), 3);
    }
}
