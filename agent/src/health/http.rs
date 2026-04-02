use async_trait::async_trait;
use nixfleet_types::health::HealthCheckResult;
use std::time::{Duration, Instant};

use super::Check;

/// Checks an HTTP endpoint by performing a GET request.
pub struct HttpChecker {
    pub url: String,
    pub timeout_secs: u64,
    pub expected_status: u16,
}

#[async_trait]
impl Check for HttpChecker {
    fn name(&self) -> &str {
        &self.url
    }

    async fn run(&self) -> HealthCheckResult {
        let check_name = self.url.clone();
        let start = Instant::now();

        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()
        {
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
