pub mod command;
pub mod config;
pub mod http;

#[cfg(target_os = "linux")]
pub mod systemd;

#[cfg(target_os = "macos")]
pub mod launchd;

use async_trait::async_trait;
use nixfleet_types::health::{HealthCheckResult, HealthReport};
use tracing::{debug, warn};

#[async_trait]
pub trait Check: Send + Sync {
    fn name(&self) -> &str;
    fn check_type(&self) -> &str;
    async fn run(&self) -> HealthCheckResult;
}

pub struct HealthRunner {
    checks: Vec<Box<dyn Check>>,
}

impl HealthRunner {
    pub fn new(checks: Vec<Box<dyn Check>>) -> Self {
        Self { checks }
    }

    pub fn from_config_path(path: &str) -> Self {
        match config::load_config(path) {
            Ok(cfg) => Self::from_config(cfg),
            Err(e) => {
                warn!(
                    health_config = path,
                    error = %e,
                    "health config not loaded; falling back to platform default"
                );
                Self::new(vec![Box::new(platform_fallback())])
            }
        }
    }

    pub fn from_config(cfg: config::HealthConfig) -> Self {
        let mut checks: Vec<Box<dyn Check>> = vec![];

        #[cfg(target_os = "linux")]
        for sc in cfg.systemd {
            for unit in sc.units {
                checks.push(Box::new(systemd::SystemdChecker { unit }));
            }
        }

        #[cfg(target_os = "macos")]
        for lc in cfg.launchd {
            for label in lc.labels {
                checks.push(Box::new(launchd::LaunchdChecker { label }));
            }
        }

        for hc in cfg.http {
            checks.push(Box::new(http::HttpChecker::new(
                hc.url,
                hc.timeout as u64,
                hc.expected_status as u16,
            )));
        }
        for cc in cfg.command {
            checks.push(Box::new(command::CommandChecker {
                name: cc.name,
                command: cc.command,
                timeout_secs: cc.timeout as u64,
            }));
        }
        if checks.is_empty() {
            checks.push(Box::new(platform_fallback()));
        }
        Self::new(checks)
    }

    pub async fn run_all(&self) -> HealthReport {
        let mut results = Vec::with_capacity(self.checks.len());
        for check in &self.checks {
            let result = check.run().await;
            debug!(
                check = check.name(),
                pass = result.is_pass(),
                "Health check"
            );
            let (duration_ms, passed) = match &result {
                HealthCheckResult::Pass { duration_ms, .. } => (*duration_ms, true),
                HealthCheckResult::Fail { duration_ms, .. } => (*duration_ms, false),
                _ => (0, false),
            };
            crate::metrics::record_health_check(
                check.name(),
                check.check_type(),
                duration_ms,
                passed,
            );
            results.push(result);
        }
        let all_passed = results.iter().all(|r| r.is_pass());
        HealthReport {
            results,
            all_passed,
            timestamp: chrono::Utc::now(),
        }
    }
}

/// Platform-appropriate fallback health check.
#[cfg(target_os = "linux")]
fn platform_fallback() -> impl Check {
    systemd::SystemdFallback
}

#[cfg(target_os = "macos")]
fn platform_fallback() -> impl Check {
    launchd::LaunchdFallback
}
