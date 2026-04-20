use async_trait::async_trait;
use nixfleet_types::health::HealthCheckResult;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use super::Check;

/// Checks an HTTP endpoint by performing a GET request.
pub struct HttpChecker {
    pub url: String,
    pub timeout_secs: u64,
    pub expected_status: u16,
    /// Cached `reqwest::Client` - built on first run and reused on every
    /// subsequent invocation so the TCP connection pool and TLS session
    /// cache survive across periodic health checks.
    client: OnceLock<reqwest::Client>,
}

impl HttpChecker {
    /// Create a new HTTP checker. The underlying `reqwest::Client` is
    /// built lazily on first use.
    pub fn new(url: String, timeout_secs: u64, expected_status: u16) -> Self {
        Self {
            url,
            timeout_secs,
            expected_status,
            client: OnceLock::new(),
        }
    }

    /// Return the cached client, building it on first call.
    fn client(&self) -> reqwest::Result<&reqwest::Client> {
        if let Some(existing) = self.client.get() {
            return Ok(existing);
        }
        let built = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()?;
        // If two threads race, one will lose - we take whichever landed.
        Ok(self.client.get_or_init(|| built))
    }
}

#[async_trait]
impl Check for HttpChecker {
    fn name(&self) -> &str {
        &self.url
    }

    fn check_type(&self) -> &str {
        "http"
    }

    async fn run(&self) -> HealthCheckResult {
        let check_name = self.url.clone();
        let start = Instant::now();

        let client = match self.client() {
            Ok(c) => c,
            Err(e) => {
                return HealthCheckResult::Fail {
                    check_name,
                    duration_ms: start.elapsed().as_millis() as u64,
                    message: format!("failed to build HTTP client: {e}"),
                };
            }
        };

        match client.get(&self.url).send().await {
            Ok(response) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                let status = response.status().as_u16();
                if status == self.expected_status {
                    HealthCheckResult::Pass {
                        check_name,
                        duration_ms,
                    }
                } else {
                    HealthCheckResult::Fail {
                        check_name,
                        duration_ms,
                        message: format!("expected status {}, got {status}", self.expected_status),
                    }
                }
            }
            Err(e) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                HealthCheckResult::Fail {
                    check_name,
                    duration_ms,
                    message: format!("HTTP request failed: {e}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// 200 from the endpoint, expected_status=200 → Pass.
    #[tokio::test]
    async fn http_checker_passes_when_status_matches() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let checker = HttpChecker::new(format!("{}/health", server.uri()), 5, 200);
        let result = checker.run().await;
        assert!(
            matches!(result, HealthCheckResult::Pass { .. }),
            "expected Pass on matching status; got {result:?}"
        );
    }

    /// 500 from the endpoint, expected_status=200 → Fail with the
    /// status-mismatch branch (NOT the network-error branch).
    #[tokio::test]
    async fn http_checker_fails_on_status_mismatch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let checker = HttpChecker::new(format!("{}/health", server.uri()), 5, 200);
        let result = checker.run().await;
        match result {
            HealthCheckResult::Fail { message, .. } => {
                assert!(
                    message.contains("expected status 200, got 500"),
                    "wrong failure message: {message:?}"
                );
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    /// Non-200 expected status: an endpoint returning 404 with
    /// expected_status=404 → Pass. Pins that the comparison is exact,
    /// not "any 2xx".
    #[tokio::test]
    async fn http_checker_passes_on_exact_non_200_match() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let checker = HttpChecker::new(format!("{}/missing", server.uri()), 5, 404);
        let result = checker.run().await;
        assert!(
            matches!(result, HealthCheckResult::Pass { .. }),
            "expected Pass on exact 404 match; got {result:?}"
        );
    }

    /// Unreachable host → Fail with the network-error branch.
    /// We use a port we know is closed by binding a tcp socket and
    /// immediately dropping its accept loop.
    #[tokio::test]
    async fn http_checker_fails_on_network_error() {
        // RFC 5737 TEST-NET-1; guaranteed not routable. Combined with a
        // short timeout this fails fast without DNS or a real connection
        // attempt to a live host.
        let checker = HttpChecker::new("http://192.0.2.1:1/".to_string(), 1, 200);
        let result = checker.run().await;
        match result {
            HealthCheckResult::Fail { message, .. } => {
                assert!(
                    message.contains("HTTP request failed"),
                    "wrong failure message: {message:?}"
                );
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }
}
