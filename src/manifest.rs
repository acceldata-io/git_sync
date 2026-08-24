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

//! Parse component manifests and turn them into validated Git reference plans.

use crate::error::GitError;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

/// A build's component manifest.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ComponentManifest {
    /// Build artifact directory represented by this manifest.
    pub build_artifact_path: Option<String>,
    /// Release version represented by this manifest.
    pub release_version: Option<String>,
    /// Repositories and exact revisions used by the build.
    pub repos: BTreeMap<String, ManifestRepository>,
}

/// One component entry in a component manifest.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ManifestRepository {
    /// GitHub repository URL.
    pub url: String,
    /// Source branch used by the build.
    pub branch: String,
    /// Exact commit used by the build.
    pub sha: String,
}

/// One validated reference creation operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestRefPlan {
    /// Manifest component name.
    pub component: String,
    /// Normalized HTTPS GitHub repository URL.
    pub repository: String,
    /// Source branch recorded in the manifest.
    pub source_branch: String,
    /// Branch or tag that will be created.
    pub target_ref: String,
    /// Exact commit for the new reference.
    pub sha: String,
}

impl ComponentManifest {
    /// Parse a component manifest from YAML.
    pub fn from_yaml(yaml: &str) -> Result<Self, GitError> {
        let mut manifest: Self = serde_yaml_ng::from_str(yaml).map_err(|e| {
            GitError::Other(format!("Failed to parse component manifest YAML: {e}"))
        })?;

        if manifest.repos.is_empty() {
            return Err(GitError::Other(
                "Component manifest contains no repositories".to_string(),
            ));
        }

        manifest.build_artifact_path = trim_optional(manifest.build_artifact_path);
        manifest.release_version = trim_optional(manifest.release_version);
        for repository in manifest.repos.values_mut() {
            repository.url = repository.url.trim().to_string();
            repository.branch = repository.branch.trim().to_string();
            repository.sha = repository.sha.trim().to_string();
        }
        Ok(manifest)
    }

    /// Load a component manifest from a local path or an HTTP(S) URL.
    pub async fn load(source: &str) -> Result<Self, GitError> {
        let source = source.trim();
        if source.is_empty() {
            return Err(GitError::Other(
                "Component manifest path or URL must not be empty".to_string(),
            ));
        }

        let yaml = if source.starts_with("https://") || source.starts_with("http://") {
            reqwest::get(source)
                .await
                .map_err(|e| GitError::Other(format!("Failed to fetch '{source}': {e}")))?
                .error_for_status()
                .map_err(|e| GitError::Other(format!("Failed to fetch '{source}': {e}")))?
                .text()
                .await
                .map_err(|e| GitError::Other(format!("Failed to read '{source}': {e}")))?
        } else {
            fs::read_to_string(Path::new(source)).map_err(|e| {
                GitError::Other(format!("Failed to read component manifest '{source}': {e}"))
            })?
        };

        Self::from_yaml(&yaml)
    }

    /// Build and validate branch creation operations for all manifest entries.
    pub fn branch_plan(
        &self,
        source_release_version: &str,
        new_version: &str,
        new_branch_prefix: &str,
    ) -> Result<Vec<ManifestRefPlan>, GitError> {
        self.build_plan(source_release_version, new_version, Some(new_branch_prefix))
    }

    /// Build and validate tag creation operations for all manifest entries.
    pub fn tag_plan(
        &self,
        source_release_version: &str,
        new_version: &str,
    ) -> Result<Vec<ManifestRefPlan>, GitError> {
        self.build_plan(source_release_version, new_version, None)
    }

    fn build_plan(
        &self,
        source_release_version: &str,
        new_version: &str,
        branch_prefix: Option<&str>,
    ) -> Result<Vec<ManifestRefPlan>, GitError> {
        let source_release_version = required_value(
            source_release_version,
            "Source release version must not be empty",
        )?;
        let new_version = required_value(new_version, "New version must not be empty")?;
        let branch_prefix = branch_prefix
            .map(|prefix| required_value(prefix, "New branch prefix must not be empty"))
            .transpose()?;

        let mut plan = Vec::with_capacity(self.repos.len());
        let mut targets: HashMap<(String, String), String> = HashMap::new();

        for (component, entry) in &self.repos {
            if component.eq_ignore_ascii_case("accelbuild") {
                eprintln!("Skipping component '{component}'");
                continue;
            }

            validate_entry(component, entry)?;
            let repository = normalize_github_url(&entry.url)?;
            let source_body = entry
                .branch
                .split_once('/')
                .map_or(entry.branch.as_str(), |(_, body)| body);

            if !source_body.contains(source_release_version) {
                return Err(GitError::Other(format!(
                    "Component '{component}' branch '{}' does not contain source release version '{source_release_version}'",
                    entry.branch
                )));
            }

            let target_body = source_body.replacen(source_release_version, new_version, 1);
            let target_ref = if let Some(prefix) = branch_prefix {
                format!("{prefix}/{target_body}")
            } else {
                format!("{target_body}-tag")
            };
            validate_ref_name(component, &target_ref)?;

            let key = (repository.clone(), target_ref.clone());
            if let Some(existing_sha) = targets.get(&key) {
                if existing_sha != &entry.sha {
                    return Err(GitError::Other(format!(
                        "Manifest target collision for {repository} '{target_ref}': {existing_sha} and {}",
                        entry.sha
                    )));
                }
                continue;
            }
            targets.insert(key, entry.sha.clone());
            plan.push(ManifestRefPlan {
                component: component.clone(),
                repository,
                source_branch: entry.branch.clone(),
                target_ref,
                sha: entry.sha.clone(),
            });
        }

        Ok(plan)
    }
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value.map(|value| value.trim().to_string())
}

fn required_value<'a>(value: &'a str, message: &str) -> Result<&'a str, GitError> {
    let value = value.trim().trim_matches('/');
    if value.is_empty() {
        Err(GitError::Other(message.to_string()))
    } else {
        Ok(value)
    }
}

fn validate_entry(component: &str, entry: &ManifestRepository) -> Result<(), GitError> {
    if entry.url.is_empty() {
        return Err(GitError::Other(format!(
            "Component '{component}' has an empty repository URL"
        )));
    }
    if entry.branch.is_empty() {
        return Err(GitError::Other(format!(
            "Component '{component}' has an empty branch"
        )));
    }
    if entry.sha.len() != 40 || !entry.sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GitError::Other(format!(
            "Component '{component}' has invalid commit SHA '{}'",
            entry.sha
        )));
    }
    Ok(())
}

fn validate_ref_name(component: &str, reference: &str) -> Result<(), GitError> {
    let invalid_character = reference
        .chars()
        .any(|character| character.is_control() || " ~^:?*[\\".contains(character));
    let invalid_shape = reference.is_empty()
        || reference == "@"
        || reference.starts_with('/')
        || reference.ends_with('/')
        || reference.ends_with('.')
        || reference.ends_with(".lock")
        || reference.contains("..")
        || reference.contains("//")
        || reference.contains("@{");
    if invalid_character || invalid_shape {
        return Err(GitError::Other(format!(
            "Component '{component}' produced invalid Git reference '{reference}'"
        )));
    }
    Ok(())
}

/// Normalize GitHub HTTPS, SSH, or owner/repository syntax to an HTTPS URL.
pub fn normalize_github_url(url: &str) -> Result<String, GitError> {
    let trimmed = url.trim().trim_end_matches('/');
    let owner_repo = if let Some(path) = trimmed.strip_prefix("git@github.com:") {
        path
    } else if let Some(path) = trimmed.strip_prefix("ssh://git@github.com/") {
        path
    } else if let Some(path) = trimmed.strip_prefix("https://github.com/") {
        path
    } else if let Some(path) = trimmed.strip_prefix("http://github.com/") {
        path
    } else {
        trimmed
    };
    let owner_repo = owner_repo.strip_suffix(".git").unwrap_or(owner_repo);
    let mut parts = owner_repo.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    let valid_part = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    };
    if !valid_part(owner) || !valid_part(repo) || parts.next().is_some() {
        return Err(GitError::InvalidRepository(url.to_string()));
    }
    Ok(format!("https://github.com/{owner}/{repo}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ODP_MANIFEST: &str = r#"
build_artifact_path: 3.3.6.5-1009
release_version: 3.3.6.5-1009
repos:
  hadoop:
    url: git@github.com:acceldata-io/hadoop.git
    branch: nightly/ODP-3.3.6.5
    sha: 68e511577e6e77e3f47427d05643d076bfe896ca
  hadoop-p1:
    url: git@github.com:acceldata-io/hadoop.git
    branch: " nightly/ODP-3.3.6.5 "
    sha: "68e511577e6e77e3f47427d05643d076bfe896ca "
  spark3_3_5_1:
    url: https://github.com/acceldata-io/spark3.git
    branch: nightly/ODP-3.5.1.3.3.6.5
    sha: e646270c889c18d24f4801a7c8617bf5e84527d9
  accelbuild:
    url: git@github.com:acceldata-io/accelbuild.git
    branch: main
    sha: 85eb777852fdae23fafec9d096e28d3842dcc98f
"#;

    const AMBARI_MANIFEST: &str = r#"
build_artifact_path: 3.0.0.2-1
release_version: 3.0.0.2-1
repos:
  ambari-infra:
    url: git@github.com:acceldata-io/odp-ambari-infra.git
    branch: rel/ODP-AMBARI-3.0.0.2-1
    sha: 2f596b80bbf467fedbb608f37d48bb7ebd6719a9
"#;

    #[test]
    fn parses_and_normalizes_odp_manifest() {
        let manifest = ComponentManifest::from_yaml(ODP_MANIFEST).unwrap();
        assert_eq!(manifest.repos.len(), 4);
        assert_eq!(manifest.repos["hadoop-p1"].branch, "nightly/ODP-3.3.6.5");
        assert_eq!(
            normalize_github_url(&manifest.repos["hadoop"].url).unwrap(),
            "https://github.com/acceldata-io/hadoop"
        );
    }

    #[tokio::test]
    async fn loads_a_local_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("component-manifest.yaml");
        fs::write(&path, AMBARI_MANIFEST).unwrap();
        let manifest = ComponentManifest::load(path.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(manifest.repos.len(), 1);
    }

    #[test]
    fn normalizes_supported_github_url_styles() {
        for url in [
            "git@github.com:acceldata-io/hadoop.git",
            "ssh://git@github.com/acceldata-io/hadoop.git",
            "https://github.com/acceldata-io/hadoop.git",
            "acceldata-io/hadoop",
        ] {
            assert_eq!(
                normalize_github_url(url).unwrap(),
                "https://github.com/acceldata-io/hadoop"
            );
        }
    }

    #[test]
    fn derives_distinct_odp_branches_and_deduplicates_aliases() {
        let manifest = ComponentManifest::from_yaml(ODP_MANIFEST).unwrap();
        let plan = manifest.branch_plan("3.3.6.5", "3.3.6.6-1", "rel").unwrap();
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].target_ref, "rel/ODP-3.3.6.6-1");
        assert_eq!(plan[1].target_ref, "rel/ODP-3.5.1.3.3.6.6-1");
    }

    #[test]
    fn derives_odp_tags() {
        let manifest = ComponentManifest::from_yaml(ODP_MANIFEST).unwrap();
        let plan = manifest.tag_plan("3.3.6.5", "3.3.6.6-1").unwrap();
        assert_eq!(plan[0].target_ref, "ODP-3.3.6.6-1-tag");
        assert_eq!(plan[1].target_ref, "ODP-3.5.1.3.3.6.6-1-tag");
    }

    #[test]
    fn derives_ambari_refs() {
        let manifest = ComponentManifest::from_yaml(AMBARI_MANIFEST).unwrap();
        let branch_plan = manifest
            .branch_plan("3.0.0.2-1", "3.0.0.2-2", "rel")
            .unwrap();
        let tag_plan = manifest.tag_plan("3.0.0.2-1", "3.0.0.2-2").unwrap();
        assert_eq!(branch_plan[0].target_ref, "rel/ODP-AMBARI-3.0.0.2-2");
        assert_eq!(tag_plan[0].target_ref, "ODP-AMBARI-3.0.0.2-2-tag");
    }

    #[test]
    fn skips_accelbuild_even_when_branch_does_not_match() {
        let manifest = ComponentManifest::from_yaml(ODP_MANIFEST).unwrap();
        let plan = manifest.branch_plan("3.3.6.5", "3.3.6.6-1", "rel").unwrap();
        assert!(
            plan.iter()
                .all(|operation| operation.component != "accelbuild")
        );
    }

    #[test]
    fn rejects_unmatched_source_version() {
        let manifest = ComponentManifest::from_yaml(AMBARI_MANIFEST).unwrap();
        let error = manifest
            .branch_plan("3.3.6.5", "3.3.6.6-1", "rel")
            .unwrap_err();
        assert!(error.to_string().contains("does not contain"));
    }

    #[test]
    fn rejects_invalid_sha() {
        let yaml = ODP_MANIFEST.replace("68e511577e6e77e3f47427d05643d076bfe896ca", "not-a-sha");
        let manifest = ComponentManifest::from_yaml(&yaml).unwrap();
        assert!(manifest.branch_plan("3.3.6.5", "3.3.6.6-1", "rel").is_err());
    }

    #[test]
    fn rejects_target_collisions_with_different_shas() {
        let yaml = ODP_MANIFEST.replace(
            "sha: \"68e511577e6e77e3f47427d05643d076bfe896ca \"",
            "sha: 1111111111111111111111111111111111111111",
        );
        let manifest = ComponentManifest::from_yaml(&yaml).unwrap();
        let error = manifest
            .branch_plan("3.3.6.5", "3.3.6.6-1", "rel")
            .unwrap_err();
        assert!(error.to_string().contains("collision"));
    }
}
