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

use crate::error::CacheError;
use crate::utils::repo::TagInfo;
use chrono::{DateTime, Utc};
use indexmap::IndexSet;
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::Duration;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct TagData {
    pub tags: IndexSet<TagInfo>,
    pub parent_urls: HashSet<String>,
}

type BranchCache = HashMap<String, CacheEntry<HashMap<String, String>>>;
type TagCache = HashMap<String, CacheEntry<TagData>>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CacheEntry<T: Clone> {
    pub data: T,
    pub fetched_at: DateTime<Utc>,
}

impl<T: Clone> CacheEntry<T> {
    fn is_stale(&self, ttl: Duration) -> bool {
        let age = Utc::now() - self.fetched_at;
        age.to_std().map(|a| a > ttl).unwrap_or(true)
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct DiskCache {
    branches: BranchCache,
    tags: TagCache,
}

#[derive(Debug)]
pub struct Cache {
    data: RwLock<DiskCache>,
    path: PathBuf,
    ttl: Duration,
    update: bool,
}

impl Cache {
    pub fn new(path: PathBuf, ttl: Duration, update: bool) -> Self {
        Self {
            data: RwLock::new(DiskCache::default()),
            path,
            ttl,
            update,
        }
    }

    pub fn from(path: &Path, ttl: Duration, update: bool) -> Result<Self, CacheError> {
        debug!("Loading cache from {}", path.display());
        let data = if path.exists() {
            let bytes = std::fs::read(path)?;
            serde_json::from_slice(&bytes)?
        } else {
            DiskCache::default()
        };
        if update {
            eprintln!("Cache will be updated this run");
        }
        Ok(Self {
            data: RwLock::new(data),
            path: path.to_owned(),
            ttl,
            update,
        })
    }

    pub fn get_branches(&self, key: &str) -> Option<HashMap<String, String>> {
        if self.update {
            return None;
        }
        let data = match self.data.read() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Warning: Failed to acquire read lock on cache: {}", e);
                return None;
            }
        };
        data.branches
            .get(key)
            .filter(|e| !e.is_stale(self.ttl))
            .map(|e| e.data.clone())
    }
    pub fn get_tags(&self, key: &str) -> Option<(IndexSet<TagInfo>, HashSet<String>)> {
        if self.update {
            return None;
        }
        let data = match self.data.read() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Warning: Failed to acquire read lock on cache: {}", e);
                return None;
            }
        };
        data.tags
            .get(key)
            .filter(|e| !e.is_stale(self.ttl))
            .map(|e| (e.data.tags.clone(), e.data.parent_urls.clone()))
    }
    pub fn set_branches(
        &self,
        key: impl AsRef<str>,
        data: HashMap<String, String>,
    ) -> Result<(), CacheError> {
        let entry = CacheEntry {
            data,
            fetched_at: Utc::now(),
        };
        let mut data = match self.data.write() {
            Ok(d) => d,
            Err(e) => return Err(CacheError::LockError(e.to_string())),
        };
        data.branches.insert(key.as_ref().to_string(), entry);

        Ok(())
    }

    pub fn remove_branch(
        &self,
        owner: &str,
        repository: &str,
        branch: &str,
    ) -> Result<(), CacheError> {
        let key = format!("{}/{}", owner, repository);
        let mut data = match self.data.write() {
            Ok(d) => d,
            Err(e) => return Err(CacheError::LockError(e.to_string())),
        };

        if let Some(entry) = data.branches.get_mut(&key) {
            entry.data.remove(branch);
        }

        Ok(())
    }

    pub fn set_tags(
        &self,
        key: impl ToString,
        data: (IndexSet<TagInfo>, HashSet<String>),
    ) -> Result<(), CacheError> {
        debug!("Writing tags to cache for {}", key.to_string());
        let entry = CacheEntry {
            data: TagData {
                tags: data.0,
                parent_urls: data.1,
            },
            fetched_at: Utc::now(),
        };
        let mut data = match self.data.write() {
            Ok(d) => d,
            Err(e) => return Err(CacheError::LockError(e.to_string())),
        };
        data.tags.insert(key.to_string(), entry);
        Ok(())
    }

    pub fn remove_tag(&self, owner: &str, repository: &str, tag: &str) -> Result<(), CacheError> {
        let key = format!("{}/{}", owner, repository);
        let mut data = match self.data.write() {
            Ok(d) => d,
            Err(e) => return Err(CacheError::LockError(e.to_string())),
        };

        if let Some(entry) = data.tags.get_mut(&key)
            && let Some(pos) = entry.data.tags.iter().position(|t| t.name == tag)
        {
            entry.data.tags.shift_remove_index(pos);
        }
        Ok(())
    }

    fn write_to_disk(&self) -> Result<(), CacheError> {
        let data = self
            .data
            .read()
            .map_err(|e| CacheError::LockError(e.to_string()))?;

        let mut branches = data.branches.clone();
        let mut tags = data.tags.clone();
        drop(data);

        branches.retain(|_, e| !e.is_stale(self.ttl));
        tags.retain(|_, e| !e.is_stale(self.ttl));
        let pruned = DiskCache { branches, tags };

        let bytes = serde_json::to_vec_pretty(&pruned)?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, &bytes)?;
        Ok(())
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        if let Err(e) = self.write_to_disk() {
            eprintln!("Warning: failed to persist cache: {e}");
        } else {
            debug!("Cache written to {}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::repo::TagType;
    use indexmap::IndexSet;
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::sync::RwLock;
    use std::time::Duration;

    // Helper functions for tests

    fn make_cache(ttl_secs: u64) -> Cache {
        Cache {
            data: RwLock::new(DiskCache::default()),
            path: PathBuf::from("/tmp/git_sync_unit_test_never_written.json"),
            ttl: Duration::from_secs(ttl_secs),
            update: false,
        }
    }

    fn stale_entry<T: Clone>(data: T, age_secs: u64) -> CacheEntry<T> {
        CacheEntry {
            data,
            fetched_at: Utc::now() - Duration::from_secs(age_secs),
        }
    }

    fn sample_branches() -> HashMap<String, String> {
        HashMap::from([
            ("main".to_string(), "2026-01-01T00:00:00Z".to_string()),
            ("develop".to_string(), "2026-01-02T00:00:00Z".to_string()),
        ])
    }

    fn sample_tag(name: &str) -> TagInfo {
        TagInfo {
            name: name.to_string(),
            tag_type: TagType::Lightweight,
            sha: "abc123".to_string(),
            url: "https://github.com/owner/repo".to_string(),
            commit_sha: None,
        }
    }

    fn sample_tag_data() -> TagData {
        let mut tags = IndexSet::new();
        tags.insert(sample_tag("v1.0"));
        tags.insert(sample_tag("v2.0"));
        TagData {
            tags,
            parent_urls: HashSet::new(),
        }
    }

    // Actual tests

    #[test]
    fn branch_entry_is_returned() {
        let cache = make_cache(3600);
        cache.set_branches("owner/repo", sample_branches()).unwrap();
        let result = cache.get_branches("owner/repo");
        assert!(result.is_some());
        let branches = result.unwrap();
        assert_eq!(branches.len(), 2);
        assert!(branches.contains_key("main"));
    }

    #[test]
    fn stale_branch_entry_is_not_returned() {
        let cache = make_cache(3600);
        // Insert entry that is greater than one hour old
        let entry = stale_entry(sample_branches(), 7200);
        cache
            .data
            .write()
            .unwrap()
            .branches
            .insert("owner/repo".to_string(), entry);

        let result = cache.get_branches("owner/repo");
        assert!(result.is_none(), "Stale entry should not be returned");
    }

    #[test]
    fn missing_branch_key_returns_none() {
        let cache = make_cache(3600);
        assert!(cache.get_branches("owner/nonexistent").is_none());
    }

    #[test]
    fn tag_entry_returned() {
        let cache = make_cache(3600);
        let (tags, parents) = (sample_tag_data().tags, sample_tag_data().parent_urls);
        cache.set_tags("owner/repo", (tags, parents)).unwrap();
        let result = cache.get_tags("owner/repo");
        assert!(result.is_some());
        let (tags, _) = result.unwrap();
        assert_eq!(tags.len(), 2);
    }

    #[test]
    fn stale_tag_entry_not_returned() {
        let cache = make_cache(3600);
        let entry = stale_entry(sample_tag_data(), 7200);
        cache
            .data
            .write()
            .unwrap()
            .tags
            .insert("owner/repo".to_string(), entry);

        assert!(
            cache.get_tags("owner/repo").is_none(),
            "Stale entry should not be returned"
        );
    }
    #[test]
    fn remove_tag_removes_specific_tag() {
        let cache = make_cache(3600);
        let data = sample_tag_data();
        cache
            .set_tags("owner/repo", (data.tags, data.parent_urls))
            .unwrap();

        cache.remove_tag("owner", "repo", "v1.0").unwrap();

        let (tags, _) = cache.get_tags("owner/repo").unwrap();
        assert!(
            !tags.iter().any(|t| t.name == "v1.0"),
            "'v1.0' should be removed"
        );
        assert!(tags.iter().any(|t| t.name == "v2.0"), "'v2.0' should exist");
    }

    #[test]
    fn remove_tag_noop_when_tag_not_in_cache() {
        let cache = make_cache(3600);
        let data = sample_tag_data();
        cache
            .set_tags("owner/repo", (data.tags, data.parent_urls))
            .unwrap();

        let result = cache.remove_tag("owner", "repo", "v99.0");
        assert!(result.is_ok());

        let (tags, _) = cache.get_tags("owner/repo").unwrap();
        assert_eq!(tags.len(), 2, "Tag count should be unchanged");
    }

    #[test]
    fn remove_tag_noop_when_repo_not_in_cache() {
        let cache = make_cache(3600);
        let result = cache.remove_tag("owner", "missing", "v1.0");
        assert!(result.is_ok());
    }
}
