use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// An immutable release manifest mapping hosts to built store paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub id: String,
    #[serde(default)]
    pub flake_ref: Option<String>,
    #[serde(default)]
    pub flake_rev: Option<String>,
    #[serde(default)]
    pub cache_url: Option<String>,
    pub host_count: usize,
    pub entries: Vec<ReleaseEntry>,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

/// A single host entry within a release.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReleaseEntry {
    pub hostname: String,
    pub store_path: String,
    pub platform: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Request body to create a release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateReleaseRequest {
    #[serde(default)]
    pub flake_ref: Option<String>,
    #[serde(default)]
    pub flake_rev: Option<String>,
    #[serde(default)]
    pub cache_url: Option<String>,
    pub entries: Vec<ReleaseEntry>,
}

/// Response after creating a release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateReleaseResponse {
    pub id: String,
    pub host_count: usize,
}

/// Diff between two releases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<ReleaseDiffEntry>,
    pub unchanged: Vec<String>,
}

/// A host whose store path changed between two releases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseDiffEntry {
    pub hostname: String,
    pub old_store_path: String,
    pub new_store_path: String,
}

#[cfg(test)]
mod tests {
    //! Only wire-contract tests live here: the `#[serde(default)]`
    //! behaviour that lets older clients omit optional fields. Plain
    //! roundtrip tests on derived serde are compile-time-enforced and
    //! exercised end-to-end by every release_scenarios.rs test.

    use super::*;

    /// ReleaseEntry.tags must default to an empty Vec when omitted.
    /// Older CLI versions did not emit the field at all.
    #[test]
    fn release_entry_empty_tags_default() {
        let json =
            r#"{"hostname":"db-01","store_path":"/nix/store/xyz","platform":"x86_64-linux"}"#;
        let entry: ReleaseEntry = serde_json::from_str(json).unwrap();
        assert!(entry.tags.is_empty());
    }

    /// CreateReleaseRequest optional fields (flake_ref, flake_rev,
    /// cache_url) must all default to None. This pins the contract
    /// for a "minimal release create" HTTP call.
    #[test]
    fn create_release_request_minimal_defaults() {
        let json = r#"{"entries":[{"hostname":"h1","store_path":"/nix/store/x","platform":"x86_64-linux"}]}"#;
        let req: CreateReleaseRequest = serde_json::from_str(json).unwrap();
        assert!(req.flake_ref.is_none());
        assert!(req.flake_rev.is_none());
        assert!(req.cache_url.is_none());
        assert_eq!(req.entries.len(), 1);
    }
}
