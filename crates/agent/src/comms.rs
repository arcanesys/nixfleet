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

        // Add custom CA certificate if configured (trusted alongside system roots)
        if let Some(ca_path) = &config.ca_cert {
            let ca_pem = std::fs::read(ca_path)
                .with_context(|| format!("failed to read CA cert: {ca_path}"))?;
            let ca = reqwest::Certificate::from_pem(&ca_pem)
                .with_context(|| format!("failed to parse CA cert: {ca_path}"))?;
            builder = builder.add_root_certificate(ca);
        }

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
    ///
    /// Returns:
    /// - `Ok(Some(DesiredGeneration))` - CP has a desired generation set
    /// - `Ok(None)` - CP returned 404 (no generation set yet, common on
    ///   fresh-DB and first-boot conditions). NOT an error.
    /// - `Err(...)` - network failure, TLS failure, or any non-404
    ///   non-2xx status from the CP.
    pub async fn get_desired_generation(
        &self,
        machine_id: &str,
    ) -> Result<Option<DesiredGeneration>> {
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
            .context("failed to reach control plane")?;

        // 404 is the documented "no generation set yet" response and is
        // expected on fresh CP DBs / first-boot. Distinguish it here so
        // the caller can log INFO instead of WARN and avoid the retry
        // storm. See `agent/src/main.rs::run_deploy_cycle`.
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }

        let resp = resp
            .error_for_status()
            .context("control plane returned error status")?;

        let desired: DesiredGeneration = resp
            .json()
            .await
            .context("failed to parse desired generation response")?;

        Ok(Some(desired))
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
            poll_interval: Duration::from_secs(60),
            retry_interval: Duration::from_secs(30),
            cache_url: None,
            db_path: ":memory:".to_string(),
            dry_run: false,
            allow_insecure: true, // Tests use http://
            ca_cert: None,
            client_cert: None,
            client_key: None,
            health_config_path: "/etc/nixfleet/health-checks.json".to_string(),
            health_interval: Duration::from_secs(60),
            tags: vec![],
            metrics_port: None,
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

    /// 404 from the CP must map to `Ok(None)` so the agent can log INFO
    /// and stay on the configured poll_interval. Without this, the agent
    /// spams WARN ("control plane returned error status") on every poll
    /// against a fresh-DB CP and busy-loops on the retry interval.
    #[tokio::test]
    async fn get_desired_generation_returns_none_on_404() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/machines/web-01/desired-generation"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let cfg = test_config(&server.uri());
        let client = Client::new(&cfg).unwrap();
        let result = client.get_desired_generation("web-01").await;

        match result {
            Ok(None) => {} // expected
            other => panic!("404 must map to Ok(None); got {other:?}"),
        }
    }

    /// 200 with a valid body must map to `Ok(Some(_))` carrying the
    /// parsed DesiredGeneration.
    #[tokio::test]
    async fn get_desired_generation_returns_some_on_200() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/machines/web-01/desired-generation"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "hash": "/nix/store/abc-system",
                "cache_url": null,
                "poll_hint": null
            })))
            .mount(&server)
            .await;

        let cfg = test_config(&server.uri());
        let client = Client::new(&cfg).unwrap();
        let result = client.get_desired_generation("web-01").await.unwrap();

        let desired = result.expect("expected Some(DesiredGeneration), got None");
        assert_eq!(desired.hash, "/nix/store/abc-system");
        assert!(desired.poll_hint.is_none());
    }

    /// Other non-2xx statuses (e.g. 500) must still surface as Err so
    /// the poll loop logs WARN and uses the retry interval.
    #[tokio::test]
    async fn get_desired_generation_returns_err_on_500() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/machines/web-01/desired-generation"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let cfg = test_config(&server.uri());
        let client = Client::new(&cfg).unwrap();
        let result = client.get_desired_generation("web-01").await;
        assert!(
            result.is_err(),
            "500 must surface as Err, not Ok(None); got {result:?}"
        );
    }
}
