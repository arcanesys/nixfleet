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
    use super::*;

    #[test]
    fn test_release_entry_roundtrip() {
        let entry = ReleaseEntry {
            hostname: "web-01".to_string(),
            store_path: "/nix/store/abc123-nixos-system-web-01".to_string(),
            platform: "x86_64-linux".to_string(),
            tags: vec!["web".to_string(), "prod".to_string()],
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: ReleaseEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn test_release_entry_empty_tags_default() {
        let json =
            r#"{"hostname":"db-01","store_path":"/nix/store/xyz","platform":"x86_64-linux"}"#;
        let entry: ReleaseEntry = serde_json::from_str(json).unwrap();
        assert!(entry.tags.is_empty());
    }

    #[test]
    fn test_create_release_request_roundtrip() {
        let req = CreateReleaseRequest {
            flake_ref: Some(".".to_string()),
            flake_rev: Some("abc123def".to_string()),
            cache_url: Some("https://attic.internal:8081/fleet".to_string()),
            entries: vec![
                ReleaseEntry {
                    hostname: "web-01".to_string(),
                    store_path: "/nix/store/aaa-nixos-system-web-01".to_string(),
                    platform: "x86_64-linux".to_string(),
                    tags: vec!["web".to_string()],
                },
                ReleaseEntry {
                    hostname: "db-01".to_string(),
                    store_path: "/nix/store/bbb-nixos-system-db-01".to_string(),
                    platform: "x86_64-linux".to_string(),
                    tags: vec!["db".to_string()],
                },
            ],
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: CreateReleaseRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.entries.len(), 2);
        assert_eq!(back.flake_rev, Some("abc123def".to_string()));
    }

    #[test]
    fn test_create_release_request_minimal() {
        let json = r#"{"entries":[{"hostname":"h1","store_path":"/nix/store/x","platform":"x86_64-linux"}]}"#;
        let req: CreateReleaseRequest = serde_json::from_str(json).unwrap();
        assert!(req.flake_ref.is_none());
        assert!(req.flake_rev.is_none());
        assert!(req.cache_url.is_none());
        assert_eq!(req.entries.len(), 1);
    }

    #[test]
    fn test_create_release_response_roundtrip() {
        let resp = CreateReleaseResponse {
            id: "rel-abc123".to_string(),
            host_count: 5,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: CreateReleaseResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "rel-abc123");
        assert_eq!(back.host_count, 5);
    }

    #[test]
    fn test_release_diff_roundtrip() {
        let diff = ReleaseDiff {
            added: vec!["new-host".to_string()],
            removed: vec!["old-host".to_string()],
            changed: vec![ReleaseDiffEntry {
                hostname: "web-01".to_string(),
                old_store_path: "/nix/store/old".to_string(),
                new_store_path: "/nix/store/new".to_string(),
            }],
            unchanged: vec!["db-01".to_string()],
        };
        let json = serde_json::to_string(&diff).unwrap();
        let back: ReleaseDiff = serde_json::from_str(&json).unwrap();
        assert_eq!(back.added, vec!["new-host"]);
        assert_eq!(back.removed, vec!["old-host"]);
        assert_eq!(back.changed.len(), 1);
        assert_eq!(back.changed[0].hostname, "web-01");
        assert_eq!(back.unchanged, vec!["db-01"]);
    }
}
