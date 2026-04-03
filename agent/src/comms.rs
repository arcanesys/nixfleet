use anyhow::{Context, Result};
use tracing::debug;

use crate::config::Config;
use crate::types::{DesiredGeneration, Report};

/// HTTP client for control plane communication.
pub struct Client {
    http: reqwest::Client,
    base_url: String,
}

impl Client {
    /// Create a new client configured for the control plane.
    pub fn new(config: &Config) -> Result<Self> {
        // Reject http:// unless --allow-insecure is set
        if !config.allow_insecure && config.control_plane_url.starts_with("http://") {
            anyhow::bail!(
                "Refusing insecure HTTP connection to control plane. \
                 Use HTTPS or set --allow-insecure for development."
            );
        }

        let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30));

        // Load client certificate for mTLS if configured
        if let (Some(cert), Some(key)) = (&config.client_cert, &config.client_key) {
            let identity = crate::tls::load_client_identity(
                std::path::Path::new(cert),
                std::path::Path::new(key),
            )?;
            builder = builder.identity(identity);
        }

        let http = builder.build().context("failed to build HTTP client")?;

        Ok(Self {
            http,
            base_url: config.control_plane_url.trim_end_matches('/').to_string(),
        })
    }

    /// Poll the control plane for the desired generation.
    ///
    /// GET /api/v1/machines/{machine_id}/desired-generation
    pub async fn get_desired_generation(&self, machine_id: &str) -> Result<DesiredGeneration> {
        let url = format!(
            "{}/api/v1/machines/{}/desired-generation",
            self.base_url, machine_id
        );
        debug!(url, "Polling for desired generation");

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("failed to reach control plane")?
            .error_for_status()
            .context("control plane returned error status")?;

        let desired: DesiredGeneration = resp
            .json()
            .await
            .context("failed to parse desired generation response")?;

        Ok(desired)
    }

    /// Report status back to the control plane.
    ///
    /// POST /api/v1/machines/{machine_id}/report
    pub async fn post_report(&self, report: &Report) -> Result<()> {
        let url = format!(
            "{}/api/v1/machines/{}/report",
            self.base_url, report.machine_id
        );
        debug!(url, "Sending report");

        self.http
            .post(&url)
            .json(report)
            .send()
            .await
            .context("failed to send report")?
            .error_for_status()
            .context("control plane rejected report")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::time::Duration;

    fn test_config(url: &str) -> Config {
        Config {
            control_plane_url: url.to_string(),
            machine_id: "web-01".to_string(),
            poll_interval: Duration::from_secs(300),
            cache_url: None,
            db_path: ":memory:".to_string(),
            dry_run: false,
            allow_insecure: true, // Tests use http://
            client_cert: None,
            client_key: None,
            health_config_path: "/etc/nixfleet/health-checks.json".to_string(),
            health_interval: Duration::from_secs(60),
            tags: vec![],
        }
    }

    #[test]
    fn test_api_url_construction_desired_generation() {
        let base = "https://fleet.example.com";
        let machine_id = "web-01";
        let url = format!("{}/api/v1/machines/{}/desired-generation", base, machine_id);
        assert_eq!(
            url,
            "https://fleet.example.com/api/v1/machines/web-01/desired-generation"
        );
    }

    #[test]
    fn test_api_url_construction_report() {
        let base = "https://fleet.example.com";
        let machine_id = "web-01";
        let url = format!("{}/api/v1/machines/{}/report", base, machine_id);
        assert_eq!(
            url,
            "https://fleet.example.com/api/v1/machines/web-01/report"
        );
    }

    #[test]
    fn test_client_new_strips_trailing_slash() {
        // URL with trailing slash should be normalized
        let config = test_config("https://fleet.example.com/");
        let client = Client::new(&config).unwrap();
        assert_eq!(client.base_url, "https://fleet.example.com");
    }

    #[test]
    fn test_client_new_no_trailing_slash() {
        let config = test_config("https://fleet.example.com");
        let client = Client::new(&config).unwrap();
        assert_eq!(client.base_url, "https://fleet.example.com");
    }

    #[test]
    fn test_client_new_multiple_trailing_slashes() {
        // trim_end_matches only removes one trailing slash
        let config = test_config("https://fleet.example.com///");
        let client = Client::new(&config).unwrap();
        // trim_end_matches('/') removes all trailing slashes
        assert_eq!(client.base_url, "https://fleet.example.com");
    }

    #[test]
    fn test_url_construction_with_different_machine_ids() {
        let base = "https://fleet.example.com";
        for machine_id in &["web-01", "dev-01", "mac-01", "srv-01"] {
            let url = format!("{}/api/v1/machines/{}/desired-generation", base, machine_id);
            assert!(url.contains(machine_id));
            assert!(url.starts_with("https://fleet.example.com/api/v1/machines/"));
            assert!(url.ends_with("/desired-generation"));
        }
    }

    #[test]
    fn test_reject_http_url_in_production() {
        let mut config = test_config("http://fleet.example.com");
        config.allow_insecure = false;
        let result = Client::new(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_allow_http_url_with_insecure_flag() {
        let config = test_config("http://fleet.example.com");
        // allow_insecure is already true in test_config
        let result = Client::new(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_https_url_always_allowed() {
        let mut config = test_config("https://fleet.example.com");
        config.allow_insecure = false;
        let result = Client::new(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_url_does_not_double_slash_with_clean_base() {
        let base = "https://fleet.example.com";
        let url = format!("{}/api/v1/machines/{}/desired-generation", base, "web-01");
        assert!(!url.contains("//api"));
    }
}
