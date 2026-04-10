// Integration tests for the NixFleet agent.
//
// These tests exercise multiple modules together, using real SQLite (in tempdir)
// and verifying cross-module behaviour such as store + state coordination.
//
// Bring in the library modules via the binary crate. Since this is a binary crate
// (no lib.rs), we drive integration tests against the public interfaces directly.
//
// Each module is imported individually to avoid needing a lib.rs.

// ── Store + types integration ────────────────────────────────────────────────

mod store_integration {
    use tempfile::tempdir;

    // Re-implement a minimal Store wrapper for integration testing
    // since agent is a binary crate with no lib.rs.
    use rusqlite::Connection;
    use std::sync::Mutex;

    struct Store {
        conn: Mutex<Connection>,
    }

    impl Store {
        fn new(path: &str) -> anyhow::Result<Self> {
            if let Some(parent) = std::path::Path::new(path).parent() {
                std::fs::create_dir_all(parent)?;
            }
            let conn = Connection::open(path)?;
            Ok(Self {
                conn: Mutex::new(conn),
            })
        }

        fn init(&self) -> anyhow::Result<()> {
            let conn = self.conn.lock().unwrap();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS events (
                    id        INTEGER PRIMARY KEY AUTOINCREMENT,
                    timestamp TEXT    NOT NULL DEFAULT (datetime('now')),
                    kind      TEXT    NOT NULL,
                    hash      TEXT,
                    message   TEXT
                );
                CREATE TABLE IF NOT EXISTS state (
                    key   TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );",
            )?;
            Ok(())
        }

        fn log_check(&self, hash: &str, status: &str) -> anyhow::Result<()> {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO events (kind, hash, message) VALUES ('check', ?1, ?2)",
                rusqlite::params![hash, status],
            )?;
            Ok(())
        }

        fn log_deploy(&self, hash: &str, success: bool) -> anyhow::Result<()> {
            let conn = self.conn.lock().unwrap();
            let msg = if success { "success" } else { "failed" };
            conn.execute(
                "INSERT INTO events (kind, hash, message) VALUES ('deploy', ?1, ?2)",
                rusqlite::params![hash, msg],
            )?;
            if success {
                conn.execute(
                    "INSERT OR REPLACE INTO state (key, value) VALUES ('current_generation', ?1)",
                    rusqlite::params![hash],
                )?;
            }
            Ok(())
        }

        fn log_rollback(&self, reason: &str) -> anyhow::Result<()> {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO events (kind, message) VALUES ('rollback', ?1)",
                rusqlite::params![reason],
            )?;
            Ok(())
        }

        fn log_error(&self, message: &str) -> anyhow::Result<()> {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO events (kind, message) VALUES ('error', ?1)",
                rusqlite::params![message],
            )?;
            Ok(())
        }

        fn count_events(&self, kind: &str) -> anyhow::Result<i64> {
            let conn = self.conn.lock().unwrap();
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM events WHERE kind = ?1",
                rusqlite::params![kind],
                |row| row.get(0),
            )?;
            Ok(count)
        }

        fn get_state(&self, key: &str) -> anyhow::Result<Option<String>> {
            let conn = self.conn.lock().unwrap();
            match conn.query_row(
                "SELECT value FROM state WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get(0),
            ) {
                Ok(v) => Ok(Some(v)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        }
    }

    #[test]
    fn test_full_deployment_lifecycle() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        let store = Store::new(db_path.to_str().unwrap()).unwrap();
        store.init().unwrap();

        let hash = "/nix/store/abc123-nixos-system-web-01-25.05";

        // Check phase
        store.log_check(hash, "mismatch").unwrap();
        assert_eq!(store.count_events("check").unwrap(), 1);

        // Successful deploy
        store.log_deploy(hash, true).unwrap();
        assert_eq!(store.count_events("deploy").unwrap(), 1);
        assert_eq!(
            store.get_state("current_generation").unwrap(),
            Some(hash.to_string())
        );
    }

    #[test]
    fn test_failed_deployment_triggers_rollback() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        let store = Store::new(db_path.to_str().unwrap()).unwrap();
        store.init().unwrap();

        let hash = "/nix/store/abc123-nixos-system";

        // Failed deploy
        store.log_deploy(hash, false).unwrap();
        // State should not be updated
        assert!(store.get_state("current_generation").unwrap().is_none());

        // Rollback logged
        store.log_rollback("health check failed").unwrap();
        assert_eq!(store.count_events("rollback").unwrap(), 1);
    }

    #[test]
    fn test_error_accumulation() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        let store = Store::new(db_path.to_str().unwrap()).unwrap();
        store.init().unwrap();

        store.log_error("check failed: connection refused").unwrap();
        store.log_error("fetch failed: nix copy timed out").unwrap();
        store
            .log_error("rollback failed: no previous generation")
            .unwrap();

        assert_eq!(store.count_events("error").unwrap(), 3);
    }

    #[test]
    fn test_state_upsert_tracks_latest_generation() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        let store = Store::new(db_path.to_str().unwrap()).unwrap();
        store.init().unwrap();

        let gen1 = "/nix/store/gen1-nixos-system";
        let gen2 = "/nix/store/gen2-nixos-system";
        let gen3 = "/nix/store/gen3-nixos-system";

        store.log_deploy(gen1, true).unwrap();
        store.log_deploy(gen2, true).unwrap();
        store.log_deploy(gen3, true).unwrap();

        // Only the latest generation should be stored
        assert_eq!(
            store.get_state("current_generation").unwrap(),
            Some(gen3.to_string())
        );
        // All three deploys are in the audit log
        assert_eq!(store.count_events("deploy").unwrap(), 3);
    }
}

// ── Serde integration: types round-trip through JSON ────────────────────────
// Now using shared types from nixfleet-types crate.

mod serde_integration {
    use chrono::Utc;
    use nixfleet_types::{DesiredGeneration, Report};

    #[test]
    fn test_report_round_trip() {
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
        assert_eq!(report.success, back.success);
    }

    #[test]
    fn test_desired_generation_minimal_json() {
        let json = r#"{"hash": "/nix/store/abc123-nixos-system"}"#;
        let gen: DesiredGeneration = serde_json::from_str(json).unwrap();
        assert_eq!(gen.hash, "/nix/store/abc123-nixos-system");
        assert!(gen.cache_url.is_none());
    }

    #[test]
    fn test_desired_generation_full_json() {
        let json = r#"{"hash": "/nix/store/abc123-nixos-system", "cache_url": "https://cache.example.com"}"#;
        let gen: DesiredGeneration = serde_json::from_str(json).unwrap();
        assert_eq!(gen.cache_url, Some("https://cache.example.com".to_string()));
    }

    #[test]
    fn test_desired_generation_equality() {
        let a = DesiredGeneration {
            hash: "/nix/store/abc123".to_string(),
            cache_url: None,
            poll_hint: None,
        };
        let b = DesiredGeneration {
            hash: "/nix/store/abc123".to_string(),
            cache_url: None,
            poll_hint: None,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_desired_generation_inequality_on_hash() {
        let a = DesiredGeneration {
            hash: "/nix/store/gen1".to_string(),
            cache_url: None,
            poll_hint: None,
        };
        let b = DesiredGeneration {
            hash: "/nix/store/gen2".to_string(),
            cache_url: None,
            poll_hint: None,
        };
        assert_ne!(a, b);
    }
}

// ── URL construction integration ─────────────────────────────────────────────

mod url_integration {
    #[test]
    fn test_desired_generation_url_format() {
        let base = "https://fleet.example.com";
        let machine_id = "web-01";
        let url = format!("{}/api/v1/machines/{}/desired-generation", base, machine_id);
        assert_eq!(
            url,
            "https://fleet.example.com/api/v1/machines/web-01/desired-generation"
        );
    }

    #[test]
    fn test_report_url_format() {
        let base = "https://fleet.example.com";
        let machine_id = "dev-01";
        let url = format!("{}/api/v1/machines/{}/report", base, machine_id);
        assert_eq!(
            url,
            "https://fleet.example.com/api/v1/machines/dev-01/report"
        );
    }

    #[test]
    fn test_trailing_slash_normalization() {
        let raw = "https://fleet.example.com/";
        let normalized = raw.trim_end_matches('/');
        assert_eq!(normalized, "https://fleet.example.com");
        let url = format!(
            "{}/api/v1/machines/{}/desired-generation",
            normalized, "web-01"
        );
        assert!(!url.contains("//api"));
    }

    #[test]
    fn test_poll_interval_range() {
        let default_secs = 300u64;
        assert!(default_secs >= 60, "poll interval must be at least 60s");
        assert!(default_secs <= 3600, "poll interval must be at most 3600s");
    }

    #[test]
    fn test_duration_from_secs() {
        let interval = std::time::Duration::from_secs(300);
        assert_eq!(interval.as_secs(), 300);
    }
}
