/// Re-export shared types from nixfleet-types.
///
/// This module previously defined DesiredGeneration, Report, and MachineStatus
/// locally. They now live in the shared `nixfleet-types` crate so the control
/// plane and agent share identical wire types.
pub use nixfleet_types::{DesiredGeneration, Report};

// MachineStatus is available via nixfleet_types::MachineStatus if needed.

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_report_serialization() {
        let report = Report {
            machine_id: "web-01".to_string(),
            current_generation: "/nix/store/abc123-nixos-system".to_string(),
            success: true,
            message: "deployed".to_string(),
            timestamp: Utc::now(),
            tags: vec![],
            health: None,
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(report.machine_id, back.machine_id);
        assert_eq!(report.current_generation, back.current_generation);
        assert_eq!(report.success, back.success);
        assert_eq!(report.message, back.message);
    }

    #[test]
    fn test_report_failure_serialization() {
        let report = Report {
            machine_id: "dev-01".to_string(),
            current_generation: "/nix/store/xyz789-nixos-system".to_string(),
            success: false,
            message: "rolled back: health check failed".to_string(),
            timestamp: Utc::now(),
            tags: vec![],
            health: None,
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(report.machine_id, back.machine_id);
        assert!(!back.success);
        assert!(back.message.contains("rolled back"));
    }

    #[test]
    fn test_desired_generation_deserialization() {
        let json = r#"{"hash": "/nix/store/abc123-nixos-system"}"#;
        let gen: DesiredGeneration = serde_json::from_str(json).unwrap();
        assert_eq!(gen.hash, "/nix/store/abc123-nixos-system");
        assert!(gen.cache_url.is_none());
    }

    #[test]
    fn test_desired_generation_with_cache_url() {
        let json = r#"{"hash": "/nix/store/abc123-nixos-system", "cache_url": "https://cache.example.com"}"#;
        let gen: DesiredGeneration = serde_json::from_str(json).unwrap();
        assert_eq!(gen.hash, "/nix/store/abc123-nixos-system");
        assert_eq!(gen.cache_url, Some("https://cache.example.com".to_string()));
    }

    #[test]
    fn test_desired_generation_serialization_roundtrip() {
        let gen = DesiredGeneration {
            hash: "/nix/store/def456-nixos-system-web-01-25.05".to_string(),
            cache_url: Some("https://cache.nixos.org".to_string()),
        };
        let json = serde_json::to_string(&gen).unwrap();
        let back: DesiredGeneration = serde_json::from_str(&json).unwrap();
        assert_eq!(gen.hash, back.hash);
        assert_eq!(gen.cache_url, back.cache_url);
    }

    #[test]
    fn test_report_json_contains_expected_fields() {
        let report = Report {
            machine_id: "mac-01".to_string(),
            current_generation: "/nix/store/ghi012-nixos-system".to_string(),
            success: true,
            message: "up-to-date".to_string(),
            timestamp: Utc::now(),
            tags: vec![],
            health: None,
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("machine_id"));
        assert!(json.contains("mac-01"));
        assert!(json.contains("success"));
        assert!(json.contains("timestamp"));
    }
}
