//! Per-host health probes (issue #86). Operator declares HTTP/TCP/exec
//! probes via `services.nixfleet-agent.healthChecks`; the agent module
//! materialises that to `/etc/nixfleet/agent/health-checks.json` and
//! passes the path via `--health-checks-config`. This module:
//!
//! 1. Loads the config file at startup (`load_config`).
//! 2. Spawns a per-probe interval ticker that runs the probe and updates
//!    the in-memory state cache.
//! 3. Exposes `latest_results` for the checkin assembler to snapshot into
//!    the wire body.
//!
//! Distinct from `compliance.rs` (which fronts the external
//! `compliance-evidence-collector` daemon for framework controls):
//! health probes run in-process, are operator-declared, and gate
//! soak promotion (not confirm). The two coexist on a host without
//! interaction.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use nixfleet_proto::agent_wire::{ProbeKind, ProbeResult, ProbeStatus};
use nixfleet_proto::compliance::GateMode;
use serde::Deserialize;
use tokio::sync::RwLock;

/// Smallest legal probe interval - protects the agent from a misconfigured
/// 0/1-second probe DOSing the local host. Below this we round up.
pub const MIN_INTERVAL_SECS: u64 = 5;

/// Per-failure cap to keep the wire body bounded - operators don't need a
/// 100-line stderr; the first ~512 bytes are the diagnostic value.
pub const FAILURE_REASON_MAX_LEN: usize = 512;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthChecksConfig {
    /// Module-level mode; mirrors `complianceGate.mode` semantics so
    /// operator UX is consistent across the two gates.
    #[serde(default = "default_mode")]
    pub mode: GateMode,
    #[serde(default)]
    pub http: Vec<HttpProbeConfig>,
    #[serde(default)]
    pub tcp: Vec<TcpProbeConfig>,
    #[serde(default)]
    pub exec: Vec<ExecProbeConfig>,
}

fn default_mode() -> GateMode {
    GateMode::Enforce
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpProbeConfig {
    pub name: String,
    pub url: String,
    #[serde(default = "default_http_status")]
    pub expect_status: u16,
    #[serde(default = "default_interval")]
    pub interval_seconds: u64,
    #[serde(default = "default_http_timeout")]
    pub timeout_seconds: u64,
}

fn default_http_status() -> u16 {
    200
}
fn default_interval() -> u64 {
    30
}
fn default_http_timeout() -> u64 {
    5
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TcpProbeConfig {
    pub name: String,
    #[serde(default = "default_tcp_host")]
    pub host: String,
    pub port: u16,
    #[serde(default = "default_interval")]
    pub interval_seconds: u64,
    #[serde(default = "default_http_timeout")]
    pub timeout_seconds: u64,
}

fn default_tcp_host() -> String {
    "127.0.0.1".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecProbeConfig {
    pub name: String,
    pub command: Vec<String>,
    #[serde(default = "default_interval")]
    pub interval_seconds: u64,
    #[serde(default = "default_exec_timeout")]
    pub timeout_seconds: u64,
}

fn default_exec_timeout() -> u64 {
    10
}

/// `Ok(None)` when the path doesn't exist (operator declared no probes  -
/// agent runs without a scheduler). Errors only on read / parse failures
/// to surface misconfiguration loudly.
pub fn load_config(path: &Path) -> Result<Option<HealthChecksConfig>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let cfg: HealthChecksConfig = serde_json::from_str(&raw)
        .with_context(|| format!("parse {} as HealthChecksConfig", path.display()))?;
    Ok(Some(cfg))
}

/// Shared probe-state cache; the scheduler writes, the checkin assembler
/// reads. Per-probe entry keyed by name. `mode` rides alongside the
/// results so the checkin assembler can attach it without a second
/// config-load.
#[derive(Debug)]
pub struct ProbeStateCache {
    inner: RwLock<Vec<ProbeResult>>,
    mode: Option<GateMode>,
}

impl Default for ProbeStateCache {
    fn default() -> Self {
        Self {
            inner: RwLock::new(Vec::new()),
            mode: None,
        }
    }
}

impl ProbeStateCache {
    pub fn new(initial: Vec<ProbeResult>, mode: GateMode) -> Self {
        Self {
            inner: RwLock::new(initial),
            mode: Some(mode),
        }
    }

    pub fn mode(&self) -> Option<GateMode> {
        self.mode
    }

    /// Snapshot of every probe's latest state, sorted by name for stable
    /// wire output. Cheap to call on every checkin tick.
    pub async fn snapshot(&self) -> Vec<ProbeResult> {
        let mut out = self.inner.read().await.clone();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Update / insert a probe's result by name. Preserves
    /// `last_pass_at` across subsequent failures so the operator can see
    /// when the probe last passed.
    pub async fn upsert(&self, mut result: ProbeResult) {
        let mut guard = self.inner.write().await;
        match guard.iter().position(|p| p.name == result.name) {
            Some(idx) => {
                if matches!(result.status, ProbeStatus::Pass) {
                    result.last_pass_at = result.last_run_at;
                } else if let Some(prev) = guard.get(idx) {
                    result.last_pass_at = prev.last_pass_at;
                }
                guard[idx] = result;
            }
            None => {
                if matches!(result.status, ProbeStatus::Pass) {
                    result.last_pass_at = result.last_run_at;
                }
                guard.push(result);
            }
        }
    }
}

/// Initial state with one Unknown entry per declared probe so the cache
/// reports the full set even before any probe has run. Soak gate
/// reads `Unknown` as non-passing - conservative semantic.
pub fn initial_results(cfg: &HealthChecksConfig) -> Vec<ProbeResult> {
    let mut out = Vec::new();
    for p in &cfg.http {
        out.push(ProbeResult {
            name: p.name.clone(),
            kind: ProbeKind::Http,
            status: ProbeStatus::Unknown,
            last_run_at: None,
            last_pass_at: None,
            failure_reason: None,
        });
    }
    for p in &cfg.tcp {
        out.push(ProbeResult {
            name: p.name.clone(),
            kind: ProbeKind::Tcp,
            status: ProbeStatus::Unknown,
            last_run_at: None,
            last_pass_at: None,
            failure_reason: None,
        });
    }
    for p in &cfg.exec {
        out.push(ProbeResult {
            name: p.name.clone(),
            kind: ProbeKind::Exec,
            status: ProbeStatus::Unknown,
            last_run_at: None,
            last_pass_at: None,
            failure_reason: None,
        });
    }
    out
}

/// Runs the probe scheduler until the process exits. Spawns one tokio
/// task per declared probe, each ticking at its own interval. `Disabled`
/// mode short-circuits - probes don't run, cache stays at Unknown. The
/// caller (main poll loop) is responsible for periodically reading
/// `cache.snapshot()` into checkin bodies.
pub async fn run_scheduler(cfg: HealthChecksConfig, cache: Arc<ProbeStateCache>) {
    if matches!(cfg.mode, GateMode::Disabled) {
        tracing::info!(
            target: "health",
            "healthChecks.mode = disabled - scheduler short-circuited",
        );
        return;
    }
    tracing::info!(
        target: "health",
        http = cfg.http.len(),
        tcp = cfg.tcp.len(),
        exec = cfg.exec.len(),
        mode = ?cfg.mode,
        "health probe scheduler starting",
    );
    for p in cfg.http {
        let cache = cache.clone();
        tokio::spawn(async move { run_http_probe(p, cache).await });
    }
    for p in cfg.tcp {
        let cache = cache.clone();
        tokio::spawn(async move { run_tcp_probe(p, cache).await });
    }
    for p in cfg.exec {
        let cache = cache.clone();
        tokio::spawn(async move { run_exec_probe(p, cache).await });
    }
}

fn clamped_interval(secs: u64) -> Duration {
    Duration::from_secs(secs.max(MIN_INTERVAL_SECS))
}

fn truncate_reason(s: String) -> String {
    if s.len() > FAILURE_REASON_MAX_LEN {
        // Take a char-boundary-safe prefix so we don't slice mid-codepoint.
        let mut end = FAILURE_REASON_MAX_LEN;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}...[truncated]", &s[..end])
    } else {
        s
    }
}

async fn run_http_probe(cfg: HttpProbeConfig, cache: Arc<ProbeStateCache>) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_seconds))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            tracing::error!(
                target: "health",
                probe = %cfg.name,
                error = %err,
                "http probe disabled - failed to build reqwest client",
            );
            return;
        }
    };
    let mut interval = tokio::time::interval(clamped_interval(cfg.interval_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let now = Utc::now();
        let result = http_check(&client, &cfg.url, cfg.expect_status, now).await;
        let result = ProbeResult {
            name: cfg.name.clone(),
            kind: ProbeKind::Http,
            ..result
        };
        cache.upsert(result).await;
    }
}

async fn http_check(
    client: &reqwest::Client,
    url: &str,
    expect_status: u16,
    now: DateTime<Utc>,
) -> ProbeResult {
    match client.get(url).send().await {
        Ok(resp) if resp.status().as_u16() == expect_status => ProbeResult {
            name: String::new(),
            kind: ProbeKind::Http,
            status: ProbeStatus::Pass,
            last_run_at: Some(now),
            last_pass_at: Some(now),
            failure_reason: None,
        },
        Ok(resp) => ProbeResult {
            name: String::new(),
            kind: ProbeKind::Http,
            status: ProbeStatus::Fail,
            last_run_at: Some(now),
            last_pass_at: None,
            failure_reason: Some(truncate_reason(format!(
                "expected {} got {}",
                expect_status,
                resp.status().as_u16(),
            ))),
        },
        Err(err) => ProbeResult {
            name: String::new(),
            kind: ProbeKind::Http,
            status: ProbeStatus::Fail,
            last_run_at: Some(now),
            last_pass_at: None,
            failure_reason: Some(truncate_reason(format!("request error: {err}"))),
        },
    }
}

async fn run_tcp_probe(cfg: TcpProbeConfig, cache: Arc<ProbeStateCache>) {
    let mut interval = tokio::time::interval(clamped_interval(cfg.interval_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let now = Utc::now();
        let result = tcp_check(&cfg.host, cfg.port, cfg.timeout_seconds, now).await;
        let result = ProbeResult {
            name: cfg.name.clone(),
            kind: ProbeKind::Tcp,
            ..result
        };
        cache.upsert(result).await;
    }
}

async fn tcp_check(host: &str, port: u16, timeout_secs: u64, now: DateTime<Utc>) -> ProbeResult {
    let target = format!("{host}:{port}");
    let connect = tokio::net::TcpStream::connect(&target);
    let outcome = tokio::time::timeout(Duration::from_secs(timeout_secs), connect).await;
    match outcome {
        Ok(Ok(_stream)) => ProbeResult {
            name: String::new(),
            kind: ProbeKind::Tcp,
            status: ProbeStatus::Pass,
            last_run_at: Some(now),
            last_pass_at: Some(now),
            failure_reason: None,
        },
        Ok(Err(err)) => ProbeResult {
            name: String::new(),
            kind: ProbeKind::Tcp,
            status: ProbeStatus::Fail,
            last_run_at: Some(now),
            last_pass_at: None,
            failure_reason: Some(truncate_reason(format!("connect: {err}"))),
        },
        Err(_) => ProbeResult {
            name: String::new(),
            kind: ProbeKind::Tcp,
            status: ProbeStatus::Fail,
            last_run_at: Some(now),
            last_pass_at: None,
            failure_reason: Some(format!("connect timeout after {timeout_secs}s")),
        },
    }
}

async fn run_exec_probe(cfg: ExecProbeConfig, cache: Arc<ProbeStateCache>) {
    let mut interval = tokio::time::interval(clamped_interval(cfg.interval_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let now = Utc::now();
        let result = exec_check(&cfg.command, cfg.timeout_seconds, now).await;
        let result = ProbeResult {
            name: cfg.name.clone(),
            kind: ProbeKind::Exec,
            ..result
        };
        cache.upsert(result).await;
    }
}

async fn exec_check(command: &[String], timeout_secs: u64, now: DateTime<Utc>) -> ProbeResult {
    if command.is_empty() {
        return ProbeResult {
            name: String::new(),
            kind: ProbeKind::Exec,
            status: ProbeStatus::Fail,
            last_run_at: Some(now),
            last_pass_at: None,
            failure_reason: Some("empty command".to_string()),
        };
    }
    let mut cmd = tokio::process::Command::new(&command[0]);
    cmd.args(&command[1..]);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let outcome = tokio::time::timeout(Duration::from_secs(timeout_secs), cmd.output()).await;
    match outcome {
        Ok(Ok(out)) if out.status.success() => ProbeResult {
            name: String::new(),
            kind: ProbeKind::Exec,
            status: ProbeStatus::Pass,
            last_run_at: Some(now),
            last_pass_at: Some(now),
            failure_reason: None,
        },
        Ok(Ok(out)) => ProbeResult {
            name: String::new(),
            kind: ProbeKind::Exec,
            status: ProbeStatus::Fail,
            last_run_at: Some(now),
            last_pass_at: None,
            failure_reason: Some(truncate_reason(format!(
                "exit {} stderr: {}",
                out.status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "killed".into()),
                String::from_utf8_lossy(&out.stderr).trim(),
            ))),
        },
        Ok(Err(err)) => ProbeResult {
            name: String::new(),
            kind: ProbeKind::Exec,
            status: ProbeStatus::Fail,
            last_run_at: Some(now),
            last_pass_at: None,
            failure_reason: Some(truncate_reason(format!("spawn: {err}"))),
        },
        Err(_) => ProbeResult {
            name: String::new(),
            kind: ProbeKind::Exec,
            status: ProbeStatus::Fail,
            last_run_at: Some(now),
            last_pass_at: None,
            failure_reason: Some(format!("exec timeout after {timeout_secs}s")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_one_http() -> HealthChecksConfig {
        HealthChecksConfig {
            mode: GateMode::Enforce,
            http: vec![HttpProbeConfig {
                name: "x".into(),
                url: "http://localhost".into(),
                expect_status: 200,
                interval_seconds: 30,
                timeout_seconds: 5,
            }],
            tcp: vec![],
            exec: vec![],
        }
    }

    #[test]
    fn initial_results_seeds_unknown_per_declared_probe() {
        let cfg = HealthChecksConfig {
            mode: GateMode::Enforce,
            http: vec![HttpProbeConfig {
                name: "h1".into(),
                url: "http://x".into(),
                expect_status: 200,
                interval_seconds: 30,
                timeout_seconds: 5,
            }],
            tcp: vec![TcpProbeConfig {
                name: "t1".into(),
                host: "127.0.0.1".into(),
                port: 22,
                interval_seconds: 30,
                timeout_seconds: 5,
            }],
            exec: vec![],
        };
        let init = initial_results(&cfg);
        assert_eq!(init.len(), 2);
        assert!(init
            .iter()
            .all(|r| matches!(r.status, ProbeStatus::Unknown)));
        assert!(init
            .iter()
            .any(|r| r.name == "h1" && matches!(r.kind, ProbeKind::Http)));
        assert!(init
            .iter()
            .any(|r| r.name == "t1" && matches!(r.kind, ProbeKind::Tcp)));
    }

    #[tokio::test]
    async fn upsert_preserves_last_pass_at_across_failure() {
        let cache = ProbeStateCache::new(initial_results(&cfg_with_one_http()), GateMode::Enforce);
        let pass_at = Utc::now();
        cache
            .upsert(ProbeResult {
                name: "x".into(),
                kind: ProbeKind::Http,
                status: ProbeStatus::Pass,
                last_run_at: Some(pass_at),
                last_pass_at: None, // upsert sets this
                failure_reason: None,
            })
            .await;
        let snap = cache.snapshot().await;
        assert_eq!(snap[0].last_pass_at, Some(pass_at));

        let fail_at = pass_at + chrono::Duration::seconds(60);
        cache
            .upsert(ProbeResult {
                name: "x".into(),
                kind: ProbeKind::Http,
                status: ProbeStatus::Fail,
                last_run_at: Some(fail_at),
                last_pass_at: None,
                failure_reason: Some("503".into()),
            })
            .await;
        let snap = cache.snapshot().await;
        assert!(matches!(snap[0].status, ProbeStatus::Fail));
        assert_eq!(
            snap[0].last_pass_at,
            Some(pass_at),
            "last_pass_at must survive subsequent failure for operator visibility",
        );
        assert_eq!(snap[0].failure_reason.as_deref(), Some("503"));
    }

    #[test]
    fn clamped_interval_lower_bounds_to_min() {
        assert_eq!(clamped_interval(0), Duration::from_secs(MIN_INTERVAL_SECS));
        assert_eq!(clamped_interval(1), Duration::from_secs(MIN_INTERVAL_SECS));
        assert_eq!(clamped_interval(30), Duration::from_secs(30));
    }

    #[test]
    fn truncate_reason_caps_at_max_len() {
        let long = "x".repeat(FAILURE_REASON_MAX_LEN + 100);
        let out = truncate_reason(long);
        assert!(out.ends_with("...[truncated]"));
        assert!(out.len() <= FAILURE_REASON_MAX_LEN + "...[truncated]".len());
    }

    #[test]
    fn truncate_reason_passthrough_short() {
        let short = "503 Service Unavailable".to_string();
        assert_eq!(truncate_reason(short.clone()), short);
    }

    #[tokio::test]
    async fn tcp_check_fails_fast_on_closed_port() {
        // High port unlikely to be bound; expect connect failure within
        // the per-probe timeout.
        let now = Utc::now();
        let r = tcp_check("127.0.0.1", 65432, 1, now).await;
        assert!(matches!(r.status, ProbeStatus::Fail));
        assert!(r.failure_reason.is_some());
    }

    #[tokio::test]
    async fn exec_check_passes_on_zero_exit() {
        let now = Utc::now();
        let r = exec_check(&["true".into()], 5, now).await;
        assert!(
            matches!(r.status, ProbeStatus::Pass),
            "status={:?}",
            r.status
        );
        assert_eq!(r.last_pass_at, Some(now));
    }

    #[tokio::test]
    async fn exec_check_fails_on_nonzero_exit() {
        let now = Utc::now();
        let r = exec_check(&["false".into()], 5, now).await;
        assert!(matches!(r.status, ProbeStatus::Fail));
        assert!(r.failure_reason.is_some());
    }

    #[tokio::test]
    async fn exec_check_fails_on_empty_command() {
        let now = Utc::now();
        let r = exec_check(&[], 5, now).await;
        assert!(matches!(r.status, ProbeStatus::Fail));
        assert_eq!(r.failure_reason.as_deref(), Some("empty command"));
    }

    #[test]
    fn load_config_parses_nix_emitted_camelcase_json() {
        // Lock the wire contract between the Nix module's
        // `pkgs.writers.writeJSON` output and the agent's serde
        // deserialiser. A drift here (rename_all attribute change,
        // field rename, mode value casing) breaks every fleet that
        // declared probes - catch it locally.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("health-checks.json");
        std::fs::write(
            &path,
            r#"{
                "mode": "enforce",
                "http": [
                    {"name": "api", "url": "http://localhost/healthz",
                     "expectStatus": 200, "intervalSeconds": 10, "timeoutSeconds": 5}
                ],
                "tcp": [
                    {"name": "ssh", "host": "127.0.0.1", "port": 22,
                     "intervalSeconds": 30, "timeoutSeconds": 5}
                ],
                "exec": [
                    {"name": "etcd", "command": ["true"],
                     "intervalSeconds": 30, "timeoutSeconds": 10}
                ]
            }"#,
        )
        .unwrap();
        let cfg = load_config(&path).unwrap().expect("present");
        assert!(matches!(cfg.mode, GateMode::Enforce));
        assert_eq!(cfg.http.len(), 1);
        assert_eq!(cfg.http[0].name, "api");
        assert_eq!(cfg.http[0].expect_status, 200);
        assert_eq!(cfg.http[0].interval_seconds, 10);
        assert_eq!(cfg.tcp.len(), 1);
        assert_eq!(cfg.tcp[0].port, 22);
        assert_eq!(cfg.exec.len(), 1);
        assert_eq!(cfg.exec[0].command, vec!["true".to_string()]);
    }

    #[test]
    fn load_config_returns_none_for_absent_path() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("nope.json");
        assert!(load_config(&absent).unwrap().is_none());
    }

    #[test]
    fn load_config_errors_loudly_on_bad_json() {
        // Operator misconfiguration should be a hard fail at agent
        // startup, not silently degraded health gating.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "{not valid").unwrap();
        let err = load_config(&path).unwrap_err();
        assert!(format!("{err:#}").contains("parse"));
    }
}
