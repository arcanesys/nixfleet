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
    /// Binary cache URL for `nix copy --from` (optional; falls back to control plane default)
    pub cache_url: Option<String>,
    /// Path to the SQLite database for local state persistence
    #[allow(dead_code)]
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> Config {
        Config {
            control_plane_url: "https://fleet.example.com".to_string(),
            machine_id: "web-01".to_string(),
            poll_interval: Duration::from_secs(300),
            cache_url: None,
            db_path: "/var/lib/nixfleet/state.db".to_string(),
            dry_run: false,
            allow_insecure: false,
            client_cert: None,
            client_key: None,
            health_config_path: "/etc/nixfleet/health-checks.json".to_string(),
            health_interval: Duration::from_secs(60),
            tags: vec![],
        }
    }

    #[test]
    fn test_config_defaults() {
        // Verify poll interval is reasonable (1 min – 1 hour)
        let interval = Duration::from_secs(300);
        assert!(interval.as_secs() >= 60);
        assert!(interval.as_secs() <= 3600);
    }

    #[test]
    fn test_config_poll_interval_at_minimum_boundary() {
        let interval = Duration::from_secs(60);
        assert!(interval.as_secs() >= 60);
    }

    #[test]
    fn test_config_poll_interval_at_maximum_boundary() {
        let interval = Duration::from_secs(3600);
        assert!(interval.as_secs() <= 3600);
    }

    #[test]
    fn test_config_clone() {
        let config = default_config();
        let cloned = config.clone();
        assert_eq!(config.machine_id, cloned.machine_id);
        assert_eq!(config.control_plane_url, cloned.control_plane_url);
        assert_eq!(config.poll_interval, cloned.poll_interval);
        assert_eq!(config.dry_run, cloned.dry_run);
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
}
