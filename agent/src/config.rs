use anyhow::Result;
use std::time::Duration;

/// Agent configuration, assembled from CLI flags and environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    /// Control plane base URL (e.g. `https://fleet.example.com`)
    pub control_plane_url: String,
    /// Machine identifier (typically the NixOS hostname)
    pub machine_id: String,
    /// How often to poll the control plane for desired generation
    pub poll_interval: Duration,
    /// How long to wait before retrying after a failed poll (short backoff
    /// so transient errors and bootstrap races recover quickly)
    pub retry_interval: Duration,
    /// Binary cache URL for `nix copy --from` (optional; falls back to control plane default)
    pub cache_url: Option<String>,
    /// Path to the SQLite database for local state persistence
    pub db_path: String,
    /// When true, fetch but do not apply generations
    pub dry_run: bool,
    /// Allow insecure HTTP connections (dev only).
    pub allow_insecure: bool,
    /// Path to client certificate PEM file (for mTLS).
    pub client_cert: Option<String>,
    /// Path to client private key PEM file (for mTLS).
    pub client_key: Option<String>,
    /// Path to the health-checks JSON configuration file.
    pub health_config_path: String,
    /// Interval between continuous health check runs.
    pub health_interval: Duration,
    /// Tags for this machine (e.g. role, environment).
    pub tags: Vec<String>,
    /// Optional port for Prometheus metrics HTTP listener.
    pub metrics_port: Option<u16>,
}

impl Config {
    /// Validate invariants that depend on multiple fields together.
    pub fn validate(&self) -> Result<()> {
        match (self.client_cert.as_ref(), self.client_key.as_ref()) {
            (Some(_), Some(_)) => Ok(()),
            (None, None) => Ok(()),
            (Some(_), None) => anyhow::bail!(
                "client_cert is set but client_key is not — mTLS requires both or neither"
            ),
            (None, Some(_)) => anyhow::bail!(
                "client_key is set but client_cert is not — mTLS requires both or neither"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> Config {
        Config {
            control_plane_url: "https://fleet.example.com".to_string(),
            machine_id: "web-01".to_string(),
            poll_interval: Duration::from_secs(300),
            retry_interval: Duration::from_secs(30),
            cache_url: None,
            db_path: "/var/lib/nixfleet/state.db".to_string(),
            dry_run: false,
            allow_insecure: false,
            client_cert: None,
            client_key: None,
            health_config_path: "/etc/nixfleet/health-checks.json".to_string(),
            health_interval: Duration::from_secs(60),
            tags: vec![],
            metrics_port: None,
        }
    }

    #[test]
    fn test_config_dry_run_default_false() {
        let config = default_config();
        assert!(!config.dry_run);
    }

    #[test]
    fn test_config_cache_url_default_none() {
        let config = default_config();
        assert!(config.cache_url.is_none());
    }

    #[test]
    fn test_config_with_cache_url() {
        let config = Config {
            cache_url: Some("https://cache.nixos.org".to_string()),
            ..default_config()
        };
        assert_eq!(
            config.cache_url,
            Some("https://cache.nixos.org".to_string())
        );
    }

    #[test]
    fn test_config_db_path_default() {
        let config = default_config();
        assert_eq!(config.db_path, "/var/lib/nixfleet/state.db");
    }

    #[test]
    fn test_config_validate_mtls_both_set() {
        let cfg = Config {
            client_cert: Some("/etc/ssl/cert.pem".into()),
            client_key: Some("/etc/ssl/key.pem".into()),
            ..default_config()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_config_validate_mtls_neither_set() {
        let cfg = default_config();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_config_validate_mtls_cert_without_key() {
        let cfg = Config {
            client_cert: Some("/etc/ssl/cert.pem".into()),
            client_key: None,
            ..default_config()
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            format!("{err}").contains("client_key is not"),
            "error should mention missing client_key"
        );
    }

    #[test]
    fn test_config_validate_mtls_key_without_cert() {
        let cfg = Config {
            client_cert: None,
            client_key: Some("/etc/ssl/key.pem".into()),
            ..default_config()
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            format!("{err}").contains("client_cert is not"),
            "error should mention missing client_cert"
        );
    }
}
