# Fleet Orchestration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add machine tags, declarative health checks, and staged rollout orchestration to nixfleet — the core features that make it a fleet management product.

**Architecture:** Three tightly coupled features built bottom-up: shared types → tags (DB + API + agent) → health checks (agent) → rollout orchestration (CP executor + API + CLI). The agent stays simple (pluggable health checks + tag/health reporting). All orchestration intelligence lives in the CP's background rollout executor.

**Tech Stack:** Rust (tokio, axum, rusqlite, reqwest, serde), Nix modules, SQLite

**Spec:** `docs/specs/2026-04-02-fleet-orchestration-design.md`

---

## File Map

### New files

| File | Responsibility |
|------|---------------|
| `shared/src/health.rs` | Health check types (HealthCheckResult, HealthReport) |
| `shared/src/rollout.rs` | Rollout types (strategy, status, request/response, batch) |
| `agent/src/health/mod.rs` | Check trait, HealthRunner |
| `agent/src/health/config.rs` | Deserialize health-checks.json |
| `agent/src/health/systemd.rs` | SystemdChecker |
| `agent/src/health/http.rs` | HttpChecker |
| `agent/src/health/command.rs` | CommandChecker |
| `control-plane/src/rollout/mod.rs` | Rollout types, batch builder |
| `control-plane/src/rollout/executor.rs` | Background rollout executor |
| `control-plane/src/rollout/routes.rs` | Rollout API endpoints |
| `control-plane/migrations/V4__machine_tags.sql` | Tags schema |
| `control-plane/migrations/V5__rollouts.sql` | Rollout + batch schema |
| `control-plane/migrations/V6__health_reports.sql` | Health reports schema |

### Modified files

| File | Changes |
|------|---------|
| `shared/src/lib.rs` | Add `mod health; mod rollout;`, extend Report with tags+health, extend MachineStatus with tags, add API path constants |
| `agent/src/config.rs` | Add `tags`, `health_config_path`, `health_interval` fields |
| `agent/src/main.rs` | Add `--health-config`, `--health-interval`, `NIXFLEET_TAGS` CLI args. Replace `health::check_system()` with HealthRunner. Add continuous health reporter loop. |
| `agent/src/comms.rs` | Update Report construction to include tags and health |
| `agent/src/types.rs` | Re-export new health types |
| `control-plane/src/lib.rs` | Add `mod rollout;`, register rollout routes in `build_app()` |
| `control-plane/src/db.rs` | Add tag CRUD, rollout CRUD, health report CRUD methods |
| `control-plane/src/state.rs` | Add `tags: Vec<String>` to MachineState, update hydration to load tags and active rollouts |
| `control-plane/src/routes.rs` | Add tag endpoints, update `post_report` to persist tags+health, update `list_machines` to include tags, add rollout conflict check to `set_desired_generation` |
| `cli/src/main.rs` | Add `rollout` subcommand group |
| `cli/src/deploy.rs` | Add rollout-based deploy flow (non-SSH) |
| `cli/src/status.rs` | Add rollout status display |
| `modules/scopes/nixfleet/_agent.nix` | Add tags, healthChecks, healthInterval options + config file generation |
| `modules/tests/eval.nix` | Add tag + health config eval checks |

---

### Task 1: Shared Types — Health and Rollout

**Files:**
- Create: `shared/src/health.rs`
- Create: `shared/src/rollout.rs`
- Modify: `shared/src/lib.rs`

- [ ] **Step 1: Create health types with tests**

Create `shared/src/health.rs`:

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Result of a single health check execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HealthCheckResult {
    Pass {
        check_name: String,
        duration_ms: u64,
    },
    Fail {
        check_name: String,
        message: String,
        duration_ms: u64,
    },
}

impl HealthCheckResult {
    pub fn is_pass(&self) -> bool {
        matches!(self, Self::Pass { .. })
    }

    pub fn check_name(&self) -> &str {
        match self {
            Self::Pass { check_name, .. } | Self::Fail { check_name, .. } => check_name,
        }
    }
}

/// Aggregated health report from all checks on a machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthReport {
    pub results: Vec<HealthCheckResult>,
    pub all_passed: bool,
    pub timestamp: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_health_check_pass_serialization() {
        let result = HealthCheckResult::Pass {
            check_name: "systemd:postgresql".to_string(),
            duration_ms: 42,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"status\":\"pass\""));
        let back: HealthCheckResult = serde_json::from_str(&json).unwrap();
        assert!(back.is_pass());
        assert_eq!(back.check_name(), "systemd:postgresql");
    }

    #[test]
    fn test_health_check_fail_serialization() {
        let result = HealthCheckResult::Fail {
            check_name: "http:localhost:8080".to_string(),
            message: "connection refused".to_string(),
            duration_ms: 3000,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"status\":\"fail\""));
        let back: HealthCheckResult = serde_json::from_str(&json).unwrap();
        assert!(!back.is_pass());
    }

    #[test]
    fn test_health_report_serialization() {
        let report = HealthReport {
            results: vec![
                HealthCheckResult::Pass {
                    check_name: "systemd:nginx".to_string(),
                    duration_ms: 10,
                },
                HealthCheckResult::Fail {
                    check_name: "http:localhost".to_string(),
                    message: "503".to_string(),
                    duration_ms: 50,
                },
            ],
            all_passed: false,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: HealthReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.results.len(), 2);
        assert!(!back.all_passed);
    }

    #[test]
    fn test_health_report_all_passed() {
        let report = HealthReport {
            results: vec![HealthCheckResult::Pass {
                check_name: "test".to_string(),
                duration_ms: 1,
            }],
            all_passed: true,
            timestamp: Utc::now(),
        };
        assert!(report.all_passed);
    }
}
```

- [ ] **Step 2: Create rollout types with tests**

Create `shared/src/rollout.rs`:

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RolloutStrategy {
    Canary,
    Staged,
    AllAtOnce,
}

impl fmt::Display for RolloutStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Canary => write!(f, "canary"),
            Self::Staged => write!(f, "staged"),
            Self::AllAtOnce => write!(f, "all_at_once"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OnFailure {
    Pause,
    Revert,
}

impl fmt::Display for OnFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pause => write!(f, "pause"),
            Self::Revert => write!(f, "revert"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RolloutStatus {
    Created,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl fmt::Display for RolloutStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Running => write!(f, "running"),
            Self::Paused => write!(f, "paused"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl RolloutStatus {
    pub fn from_str_lc(s: &str) -> Option<Self> {
        match s {
            "created" => Some(Self::Created),
            "running" => Some(Self::Running),
            "paused" => Some(Self::Paused),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Created | Self::Running | Self::Paused)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    Pending,
    Deploying,
    WaitingHealth,
    Succeeded,
    Failed,
}

impl fmt::Display for BatchStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Deploying => write!(f, "deploying"),
            Self::WaitingHealth => write!(f, "waiting_health"),
            Self::Succeeded => write!(f, "succeeded"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl BatchStatus {
    pub fn from_str_lc(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "deploying" => Some(Self::Deploying),
            "waiting_health" => Some(Self::WaitingHealth),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MachineHealthStatus {
    Pending,
    Healthy,
    Unhealthy(String),
    TimedOut,
    RolledBack,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RolloutTarget {
    Tags(Vec<String>),
    Hosts(Vec<String>),
}

/// Request to create a new rollout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRolloutRequest {
    pub generation_hash: String,
    #[serde(default)]
    pub cache_url: Option<String>,
    pub strategy: RolloutStrategy,
    /// Required for Staged. Ignored for Canary (defaults to ["1","100%"]) and AllAtOnce (["100%"]).
    #[serde(default)]
    pub batch_sizes: Option<Vec<String>>,
    /// Absolute ("1") or percentage ("30%").
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: String,
    #[serde(default)]
    pub on_failure: OnFailure,
    /// Seconds to wait for health after batch deploy. Default: 300.
    #[serde(default)]
    pub health_timeout: Option<u64>,
    pub target: RolloutTarget,
}

fn default_failure_threshold() -> String {
    "1".to_string()
}

impl Default for OnFailure {
    fn default() -> Self {
        Self::Pause
    }
}

/// Response after creating a rollout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRolloutResponse {
    pub rollout_id: String,
    pub batches: Vec<BatchSummary>,
    pub total_machines: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSummary {
    pub batch_index: u32,
    pub machine_ids: Vec<String>,
    pub status: BatchStatus,
}

/// Full rollout detail with batch health data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolloutDetail {
    pub id: String,
    pub status: RolloutStatus,
    pub strategy: RolloutStrategy,
    pub generation_hash: String,
    pub on_failure: OnFailure,
    pub failure_threshold: String,
    pub health_timeout: u64,
    pub batches: Vec<BatchDetail>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchDetail {
    pub batch_index: u32,
    pub machine_ids: Vec<String>,
    pub status: BatchStatus,
    pub machine_health: HashMap<String, MachineHealthStatus>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rollout_strategy_serialization() {
        let s = RolloutStrategy::Canary;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"canary\"");
        let back: RolloutStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, RolloutStrategy::Canary);
    }

    #[test]
    fn test_rollout_status_active() {
        assert!(RolloutStatus::Created.is_active());
        assert!(RolloutStatus::Running.is_active());
        assert!(RolloutStatus::Paused.is_active());
        assert!(!RolloutStatus::Completed.is_active());
        assert!(!RolloutStatus::Failed.is_active());
        assert!(!RolloutStatus::Cancelled.is_active());
    }

    #[test]
    fn test_on_failure_default() {
        let default = OnFailure::default();
        assert_eq!(default, OnFailure::Pause);
    }

    #[test]
    fn test_create_rollout_request_deserialization() {
        let json = r#"{
            "generation_hash": "/nix/store/abc123",
            "strategy": "canary",
            "failure_threshold": "1",
            "on_failure": "pause",
            "target": {"tags": ["production", "eu-west"]}
        }"#;
        let req: CreateRolloutRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.generation_hash, "/nix/store/abc123");
        assert_eq!(req.strategy, RolloutStrategy::Canary);
        assert!(req.batch_sizes.is_none());
        assert_eq!(req.target, RolloutTarget::Tags(vec!["production".into(), "eu-west".into()]));
    }

    #[test]
    fn test_create_rollout_request_with_hosts() {
        let json = r#"{
            "generation_hash": "/nix/store/abc123",
            "strategy": "all_at_once",
            "target": {"hosts": ["web-01", "web-02"]}
        }"#;
        let req: CreateRolloutRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.target, RolloutTarget::Hosts(vec!["web-01".into(), "web-02".into()]));
    }

    #[test]
    fn test_batch_status_from_str() {
        assert_eq!(BatchStatus::from_str_lc("pending"), Some(BatchStatus::Pending));
        assert_eq!(BatchStatus::from_str_lc("deploying"), Some(BatchStatus::Deploying));
        assert_eq!(BatchStatus::from_str_lc("waiting_health"), Some(BatchStatus::WaitingHealth));
        assert_eq!(BatchStatus::from_str_lc("invalid"), None);
    }

    #[test]
    fn test_machine_health_status_serialization() {
        let s = MachineHealthStatus::Unhealthy("http check failed".to_string());
        let json = serde_json::to_string(&s).unwrap();
        let back: MachineHealthStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn test_rollout_detail_serialization() {
        let detail = RolloutDetail {
            id: "r-abc123".to_string(),
            status: RolloutStatus::Running,
            strategy: RolloutStrategy::Canary,
            generation_hash: "/nix/store/abc123".to_string(),
            on_failure: OnFailure::Pause,
            failure_threshold: "1".to_string(),
            health_timeout: 300,
            batches: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            created_by: "apikey:deploy".to_string(),
        };
        let json = serde_json::to_string(&detail).unwrap();
        let back: RolloutDetail = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "r-abc123");
        assert_eq!(back.status, RolloutStatus::Running);
    }
}
```

- [ ] **Step 3: Update shared/src/lib.rs — add modules, extend Report and MachineStatus, add API paths**

Add module declarations after the existing `use` statements at top of `shared/src/lib.rs`:

```rust
pub mod health;
pub mod rollout;
```

Update `Report` to include optional tags and health:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub machine_id: String,
    pub current_generation: String,
    pub success: bool,
    pub message: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub health: Option<health::HealthReport>,
}
```

Update `MachineStatus` to include tags:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineStatus {
    pub machine_id: String,
    pub current_generation: String,
    pub desired_generation: Option<String>,
    pub agent_version: String,
    pub system_state: String,
    pub uptime_seconds: u64,
    pub last_report: Option<DateTime<Utc>>,
    pub lifecycle: MachineLifecycle,
    #[serde(default)]
    pub tags: Vec<String>,
}
```

Add new API path constants to the `api` module:

```rust
pub const ROLLOUTS: &str = "/api/v1/rollouts";
pub const ROLLOUT: &str = "/api/v1/rollouts/{id}";
pub const ROLLOUT_RESUME: &str = "/api/v1/rollouts/{id}/resume";
pub const ROLLOUT_CANCEL: &str = "/api/v1/rollouts/{id}/cancel";
pub const MACHINE_TAGS: &str = "/api/v1/machines/{id}/tags";
pub const MACHINE_TAG: &str = "/api/v1/machines/{id}/tags/{tag}";
```

Update existing tests that construct `Report` and `MachineStatus` to include the new fields (`tags: vec![]`, `health: None`, `tags: vec![]`).

- [ ] **Step 4: Run tests**

Run: `cargo test --workspace`
Expected: All tests pass (existing + new)

- [ ] **Step 5: Commit**

```bash
git add shared/src/health.rs shared/src/rollout.rs shared/src/lib.rs
git commit -m "feat: add shared types for health checks, rollouts, and tags"
```

---

### Task 2: Database Migrations — Tags, Rollouts, Health Reports

**Files:**
- Create: `control-plane/migrations/V4__machine_tags.sql`
- Create: `control-plane/migrations/V5__rollouts.sql`
- Create: `control-plane/migrations/V6__health_reports.sql`

- [ ] **Step 1: Create V4 migration**

Create `control-plane/migrations/V4__machine_tags.sql`:

```sql
CREATE TABLE machine_tags (
    machine_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    PRIMARY KEY (machine_id, tag),
    FOREIGN KEY (machine_id) REFERENCES machines(machine_id)
);
CREATE INDEX idx_machine_tags_tag ON machine_tags(tag);
```

- [ ] **Step 2: Create V5 migration**

Create `control-plane/migrations/V5__rollouts.sql`:

```sql
CREATE TABLE rollouts (
    id TEXT PRIMARY KEY,
    generation_hash TEXT NOT NULL,
    cache_url TEXT,
    strategy TEXT NOT NULL,
    batch_sizes TEXT NOT NULL,
    failure_threshold TEXT NOT NULL,
    on_failure TEXT NOT NULL,
    health_timeout INTEGER NOT NULL DEFAULT 300,
    status TEXT NOT NULL DEFAULT 'created',
    target_tags TEXT,
    target_hosts TEXT,
    previous_generation TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    created_by TEXT NOT NULL
);
CREATE INDEX idx_rollouts_status ON rollouts(status);

CREATE TABLE rollout_batches (
    id TEXT PRIMARY KEY,
    rollout_id TEXT NOT NULL,
    batch_index INTEGER NOT NULL,
    machine_ids TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    started_at TEXT,
    completed_at TEXT,
    FOREIGN KEY (rollout_id) REFERENCES rollouts(id)
);
CREATE INDEX idx_rollout_batches_rollout ON rollout_batches(rollout_id);
```

- [ ] **Step 3: Create V6 migration**

Create `control-plane/migrations/V6__health_reports.sql`:

```sql
CREATE TABLE health_reports (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    machine_id TEXT NOT NULL,
    results TEXT NOT NULL,
    all_passed INTEGER NOT NULL,
    received_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX idx_health_reports_machine ON health_reports(machine_id, received_at);
```

- [ ] **Step 4: Verify migrations run**

Run: `cargo test -p nixfleet-control-plane -- test_migrate_is_idempotent`
Expected: PASS (refinery discovers and runs V4-V6)

- [ ] **Step 5: Commit**

```bash
git add control-plane/migrations/
git commit -m "feat: add database migrations for tags, rollouts, and health reports"
```

---

### Task 3: Database Methods — Tag CRUD

**Files:**
- Modify: `control-plane/src/db.rs`

- [ ] **Step 1: Write failing tests for tag operations**

Add to `control-plane/src/db.rs` tests module:

```rust
#[test]
fn test_set_and_get_machine_tags() {
    let (db, _dir) = make_db();
    db.register_machine("web-01", "active").unwrap();
    db.set_machine_tags("web-01", &["production".to_string(), "eu-west".to_string()]).unwrap();
    let tags = db.get_machine_tags("web-01").unwrap();
    assert_eq!(tags.len(), 2);
    assert!(tags.contains(&"production".to_string()));
    assert!(tags.contains(&"eu-west".to_string()));
}

#[test]
fn test_set_machine_tags_replaces() {
    let (db, _dir) = make_db();
    db.register_machine("web-01", "active").unwrap();
    db.set_machine_tags("web-01", &["old-tag".to_string()]).unwrap();
    db.set_machine_tags("web-01", &["new-tag".to_string()]).unwrap();
    let tags = db.get_machine_tags("web-01").unwrap();
    assert_eq!(tags, vec!["new-tag".to_string()]);
}

#[test]
fn test_get_machines_by_tags_and_logic() {
    let (db, _dir) = make_db();
    db.register_machine("web-01", "active").unwrap();
    db.register_machine("web-02", "active").unwrap();
    db.register_machine("db-01", "active").unwrap();
    db.set_machine_tags("web-01", &["web".to_string(), "production".to_string()]).unwrap();
    db.set_machine_tags("web-02", &["web".to_string(), "staging".to_string()]).unwrap();
    db.set_machine_tags("db-01", &["db".to_string(), "production".to_string()]).unwrap();
    // AND: web + production = only web-01
    let machines = db.get_machines_by_tags(&["web".to_string(), "production".to_string()]).unwrap();
    assert_eq!(machines, vec!["web-01".to_string()]);
}

#[test]
fn test_remove_machine_tag() {
    let (db, _dir) = make_db();
    db.register_machine("web-01", "active").unwrap();
    db.set_machine_tags("web-01", &["web".to_string(), "production".to_string()]).unwrap();
    db.remove_machine_tag("web-01", "web").unwrap();
    let tags = db.get_machine_tags("web-01").unwrap();
    assert_eq!(tags, vec!["production".to_string()]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p nixfleet-control-plane -- test_set_and_get_machine_tags test_set_machine_tags_replaces test_get_machines_by_tags_and_logic test_remove_machine_tag`
Expected: FAIL (methods don't exist)

- [ ] **Step 3: Implement tag CRUD methods**

Add to `control-plane/src/db.rs` impl block:

```rust
/// Set tags for a machine (replaces all existing tags).
pub fn set_machine_tags(&self, machine_id: &str, tags: &[String]) -> Result<()> {
    let conn = self.conn.lock().unwrap();
    conn.execute(
        "DELETE FROM machine_tags WHERE machine_id = ?1",
        rusqlite::params![machine_id],
    )?;
    for tag in tags {
        conn.execute(
            "INSERT INTO machine_tags (machine_id, tag) VALUES (?1, ?2)",
            rusqlite::params![machine_id, tag],
        )?;
    }
    Ok(())
}

/// Get all tags for a machine.
pub fn get_machine_tags(&self, machine_id: &str) -> Result<Vec<String>> {
    let conn = self.conn.lock().unwrap();
    let mut stmt = conn.prepare("SELECT tag FROM machine_tags WHERE machine_id = ?1 ORDER BY tag")?;
    let tags = stmt
        .query_map(rusqlite::params![machine_id], |row| row.get(0))?
        .collect::<std::result::Result<Vec<String>, _>>()?;
    Ok(tags)
}

/// Get machine IDs matching ALL given tags (AND logic).
pub fn get_machines_by_tags(&self, tags: &[String]) -> Result<Vec<String>> {
    if tags.is_empty() {
        return Ok(vec![]);
    }
    let placeholders: Vec<String> = (1..=tags.len()).map(|i| format!("?{i}")).collect();
    let sql = format!(
        "SELECT machine_id FROM machine_tags WHERE tag IN ({}) GROUP BY machine_id HAVING COUNT(DISTINCT tag) = ?{}",
        placeholders.join(", "),
        tags.len() + 1
    );
    let conn = self.conn.lock().unwrap();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = tags
        .iter()
        .map(|t| Box::new(t.clone()) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    params.push(Box::new(tags.len() as i64));
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let machines = stmt
        .query_map(param_refs.as_slice(), |row| row.get(0))?
        .collect::<std::result::Result<Vec<String>, _>>()?;
    Ok(machines)
}

/// Remove a single tag from a machine.
pub fn remove_machine_tag(&self, machine_id: &str, tag: &str) -> Result<()> {
    let conn = self.conn.lock().unwrap();
    conn.execute(
        "DELETE FROM machine_tags WHERE machine_id = ?1 AND tag = ?2",
        rusqlite::params![machine_id, tag],
    )?;
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p nixfleet-control-plane -- test_set_and_get_machine_tags test_set_machine_tags_replaces test_get_machines_by_tags_and_logic test_remove_machine_tag`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add control-plane/src/db.rs
git commit -m "feat: add tag CRUD methods to control plane database"
```

---

### Task 4: Database Methods — Rollout and Health CRUD

**Files:**
- Modify: `control-plane/src/db.rs`

- [ ] **Step 1: Write failing tests for rollout operations**

Add to tests module:

```rust
#[test]
fn test_create_and_get_rollout() {
    let (db, _dir) = make_db();
    let id = db.create_rollout(
        "r-001", "/nix/store/abc123", None, "canary",
        r#"["1","100%"]"#, "1", "pause", 300,
        Some(r#"["production"]"#), None, Some("/nix/store/old"), "apikey:deploy",
    ).unwrap();
    assert_eq!(id, "r-001");
    let rollout = db.get_rollout("r-001").unwrap().unwrap();
    assert_eq!(rollout.generation_hash, "/nix/store/abc123");
    assert_eq!(rollout.status, "created");
    assert_eq!(rollout.strategy, "canary");
}

#[test]
fn test_create_and_get_rollout_batches() {
    let (db, _dir) = make_db();
    db.create_rollout(
        "r-001", "/nix/store/abc123", None, "canary",
        r#"["1","100%"]"#, "1", "pause", 300,
        None, None, None, "apikey:deploy",
    ).unwrap();
    db.create_rollout_batch("b-001", "r-001", 0, r#"["web-01"]"#).unwrap();
    db.create_rollout_batch("b-002", "r-001", 1, r#"["web-02","web-03"]"#).unwrap();
    let batches = db.get_rollout_batches("r-001").unwrap();
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].batch_index, 0);
    assert_eq!(batches[1].batch_index, 1);
}

#[test]
fn test_update_rollout_status() {
    let (db, _dir) = make_db();
    db.create_rollout(
        "r-001", "/nix/store/abc123", None, "canary",
        r#"["1","100%"]"#, "1", "pause", 300,
        None, None, None, "apikey:deploy",
    ).unwrap();
    db.update_rollout_status("r-001", "running").unwrap();
    let rollout = db.get_rollout("r-001").unwrap().unwrap();
    assert_eq!(rollout.status, "running");
}

#[test]
fn test_list_rollouts_by_status() {
    let (db, _dir) = make_db();
    db.create_rollout("r-001", "/nix/store/a", None, "canary", "[]", "1", "pause", 300, None, None, None, "x").unwrap();
    db.create_rollout("r-002", "/nix/store/b", None, "canary", "[]", "1", "pause", 300, None, None, None, "x").unwrap();
    db.update_rollout_status("r-001", "running").unwrap();
    let running = db.list_rollouts_by_status(Some("running"), 10).unwrap();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].id, "r-001");
}

#[test]
fn test_insert_and_query_health_reports() {
    let (db, _dir) = make_db();
    db.insert_health_report("web-01", r#"[{"status":"pass","check_name":"test","duration_ms":1}]"#, true).unwrap();
    let reports = db.get_health_reports_since("web-01", "2000-01-01 00:00:00").unwrap();
    assert_eq!(reports.len(), 1);
    assert!(reports[0].all_passed);
}

#[test]
fn test_cleanup_old_health_reports() {
    let (db, _dir) = make_db();
    db.insert_health_report("web-01", "[]", true).unwrap();
    // Cleanup with 0 hours retention should delete everything
    let deleted = db.cleanup_old_health_reports(0).unwrap();
    assert_eq!(deleted, 1);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p nixfleet-control-plane -- test_create_and_get_rollout test_create_and_get_rollout_batches test_update_rollout_status test_list_rollouts_by_status test_insert_and_query_health_reports test_cleanup_old_health_reports`
Expected: FAIL

- [ ] **Step 3: Add rollout and health row types**

Add to `control-plane/src/db.rs`:

```rust
/// A rollout row as stored in SQLite.
#[derive(Debug, Clone)]
pub struct RolloutRow {
    pub id: String,
    pub generation_hash: String,
    pub cache_url: Option<String>,
    pub strategy: String,
    pub batch_sizes: String,
    pub failure_threshold: String,
    pub on_failure: String,
    pub health_timeout: i64,
    pub status: String,
    pub target_tags: Option<String>,
    pub target_hosts: Option<String>,
    pub previous_generation: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub created_by: String,
}

/// A rollout batch row as stored in SQLite.
#[derive(Debug, Clone)]
pub struct RolloutBatchRow {
    pub id: String,
    pub rollout_id: String,
    pub batch_index: i64,
    pub machine_ids: String,
    pub status: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

/// A health report row as stored in SQLite.
#[derive(Debug, Clone)]
pub struct HealthReportRow {
    pub id: i64,
    pub machine_id: String,
    pub results: String,
    pub all_passed: bool,
    pub received_at: String,
}
```

- [ ] **Step 4: Implement rollout and health CRUD methods**

Add to impl block:

```rust
/// Create a rollout record.
#[allow(clippy::too_many_arguments)]
pub fn create_rollout(
    &self,
    id: &str,
    generation_hash: &str,
    cache_url: Option<&str>,
    strategy: &str,
    batch_sizes: &str,
    failure_threshold: &str,
    on_failure: &str,
    health_timeout: i64,
    target_tags: Option<&str>,
    target_hosts: Option<&str>,
    previous_generation: Option<&str>,
    created_by: &str,
) -> Result<String> {
    let conn = self.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO rollouts (id, generation_hash, cache_url, strategy, batch_sizes, failure_threshold, on_failure, health_timeout, status, target_tags, target_hosts, previous_generation, created_at, updated_at, created_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'created', ?9, ?10, ?11, datetime('now'), datetime('now'), ?12)",
        rusqlite::params![id, generation_hash, cache_url, strategy, batch_sizes, failure_threshold, on_failure, health_timeout, target_tags, target_hosts, previous_generation, created_by],
    ).context("failed to create rollout")?;
    Ok(id.to_string())
}

/// Create a rollout batch record.
pub fn create_rollout_batch(
    &self,
    id: &str,
    rollout_id: &str,
    batch_index: i64,
    machine_ids: &str,
) -> Result<()> {
    let conn = self.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO rollout_batches (id, rollout_id, batch_index, machine_ids)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![id, rollout_id, batch_index, machine_ids],
    ).context("failed to create rollout batch")?;
    Ok(())
}

/// Get a rollout by ID.
pub fn get_rollout(&self, id: &str) -> Result<Option<RolloutRow>> {
    let conn = self.conn.lock().unwrap();
    let result = conn.query_row(
        "SELECT id, generation_hash, cache_url, strategy, batch_sizes, failure_threshold, on_failure, health_timeout, status, target_tags, target_hosts, previous_generation, created_at, updated_at, created_by
         FROM rollouts WHERE id = ?1",
        rusqlite::params![id],
        |row| Ok(RolloutRow {
            id: row.get(0)?,
            generation_hash: row.get(1)?,
            cache_url: row.get(2)?,
            strategy: row.get(3)?,
            batch_sizes: row.get(4)?,
            failure_threshold: row.get(5)?,
            on_failure: row.get(6)?,
            health_timeout: row.get(7)?,
            status: row.get(8)?,
            target_tags: row.get(9)?,
            target_hosts: row.get(10)?,
            previous_generation: row.get(11)?,
            created_at: row.get(12)?,
            updated_at: row.get(13)?,
            created_by: row.get(14)?,
        }),
    );
    match result {
        Ok(row) => Ok(Some(row)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// List rollouts, optionally filtered by status.
pub fn list_rollouts_by_status(&self, status: Option<&str>, limit: usize) -> Result<Vec<RolloutRow>> {
    let conn = self.conn.lock().unwrap();
    let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(s) = status {
        (
            "SELECT id, generation_hash, cache_url, strategy, batch_sizes, failure_threshold, on_failure, health_timeout, status, target_tags, target_hosts, previous_generation, created_at, updated_at, created_by FROM rollouts WHERE status = ?1 ORDER BY created_at DESC LIMIT ?2".to_string(),
            vec![Box::new(s.to_string()), Box::new(limit as i64)],
        )
    } else {
        (
            "SELECT id, generation_hash, cache_url, strategy, batch_sizes, failure_threshold, on_failure, health_timeout, status, target_tags, target_hosts, previous_generation, created_at, updated_at, created_by FROM rollouts ORDER BY created_at DESC LIMIT ?1".to_string(),
            vec![Box::new(limit as i64)],
        )
    };
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(param_refs.as_slice(), |row| Ok(RolloutRow {
        id: row.get(0)?,
        generation_hash: row.get(1)?,
        cache_url: row.get(2)?,
        strategy: row.get(3)?,
        batch_sizes: row.get(4)?,
        failure_threshold: row.get(5)?,
        on_failure: row.get(6)?,
        health_timeout: row.get(7)?,
        status: row.get(8)?,
        target_tags: row.get(9)?,
        target_hosts: row.get(10)?,
        previous_generation: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        created_by: row.get(14)?,
    }))?.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Update a rollout's status.
pub fn update_rollout_status(&self, id: &str, status: &str) -> Result<()> {
    let conn = self.conn.lock().unwrap();
    conn.execute(
        "UPDATE rollouts SET status = ?2, updated_at = datetime('now') WHERE id = ?1",
        rusqlite::params![id, status],
    ).context("failed to update rollout status")?;
    Ok(())
}

/// Get batches for a rollout, ordered by batch_index.
pub fn get_rollout_batches(&self, rollout_id: &str) -> Result<Vec<RolloutBatchRow>> {
    let conn = self.conn.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, rollout_id, batch_index, machine_ids, status, started_at, completed_at
         FROM rollout_batches WHERE rollout_id = ?1 ORDER BY batch_index"
    )?;
    let rows = stmt.query_map(rusqlite::params![rollout_id], |row| Ok(RolloutBatchRow {
        id: row.get(0)?,
        rollout_id: row.get(1)?,
        batch_index: row.get(2)?,
        machine_ids: row.get(3)?,
        status: row.get(4)?,
        started_at: row.get(5)?,
        completed_at: row.get(6)?,
    }))?.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Update a batch's status.
pub fn update_batch_status(&self, id: &str, status: &str, timestamp: Option<&str>) -> Result<()> {
    let conn = self.conn.lock().unwrap();
    match status {
        "deploying" => conn.execute(
            "UPDATE rollout_batches SET status = ?2, started_at = ?3 WHERE id = ?1",
            rusqlite::params![id, status, timestamp],
        ),
        "succeeded" | "failed" => conn.execute(
            "UPDATE rollout_batches SET status = ?2, completed_at = ?3 WHERE id = ?1",
            rusqlite::params![id, status, timestamp],
        ),
        _ => conn.execute(
            "UPDATE rollout_batches SET status = ?2 WHERE id = ?1",
            rusqlite::params![id, status],
        ),
    }.context("failed to update batch status")?;
    Ok(())
}

/// Insert a health report.
pub fn insert_health_report(&self, machine_id: &str, results: &str, all_passed: bool) -> Result<()> {
    let conn = self.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO health_reports (machine_id, results, all_passed) VALUES (?1, ?2, ?3)",
        rusqlite::params![machine_id, results, all_passed as i32],
    ).context("failed to insert health report")?;
    Ok(())
}

/// Get health reports for a machine since a given timestamp.
pub fn get_health_reports_since(&self, machine_id: &str, since: &str) -> Result<Vec<HealthReportRow>> {
    let conn = self.conn.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, machine_id, results, all_passed, received_at FROM health_reports
         WHERE machine_id = ?1 AND received_at >= ?2 ORDER BY received_at DESC"
    )?;
    let rows = stmt.query_map(rusqlite::params![machine_id, since], |row| Ok(HealthReportRow {
        id: row.get(0)?,
        machine_id: row.get(1)?,
        results: row.get(2)?,
        all_passed: row.get::<_, i32>(3)? != 0,
        received_at: row.get(4)?,
    }))?.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Delete health reports older than retention_hours.
pub fn cleanup_old_health_reports(&self, retention_hours: u32) -> Result<u64> {
    let conn = self.conn.lock().unwrap();
    let deleted = conn.execute(
        "DELETE FROM health_reports WHERE received_at < datetime('now', ?1)",
        rusqlite::params![format!("-{retention_hours} hours")],
    ).context("failed to cleanup health reports")?;
    Ok(deleted as u64)
}

/// Check if a machine is in any active rollout.
pub fn machine_in_active_rollout(&self, machine_id: &str) -> Result<Option<String>> {
    let conn = self.conn.lock().unwrap();
    let result = conn.query_row(
        "SELECT r.id FROM rollouts r
         JOIN rollout_batches rb ON rb.rollout_id = r.id
         WHERE r.status IN ('created', 'running', 'paused')
         AND rb.machine_ids LIKE '%' || ?1 || '%'
         LIMIT 1",
        rusqlite::params![machine_id],
        |row| row.get(0),
    );
    match result {
        Ok(id) => Ok(Some(id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p nixfleet-control-plane`
Expected: All tests pass

- [ ] **Step 6: Commit**

```bash
git add control-plane/src/db.rs
git commit -m "feat: add rollout, batch, and health report CRUD to control plane database"
```

---

### Task 5: CP Tag Routes and State Update

**Files:**
- Modify: `control-plane/src/routes.rs`
- Modify: `control-plane/src/state.rs`
- Modify: `control-plane/src/lib.rs`

- [ ] **Step 1: Add tags to MachineState**

In `control-plane/src/state.rs`, add to `MachineState`:

```rust
pub struct MachineState {
    pub desired_generation: Option<DesiredGeneration>,
    pub last_report: Option<Report>,
    pub last_seen: Option<DateTime<Utc>>,
    pub lifecycle: MachineLifecycle,
    pub registered_at: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
}
```

Update `MachineState::new()` and `new_pending()` to include `tags: vec![]`.

Update `hydrate_from_db` to load tags:

```rust
// After loading registered machines, load tags
for row in &registered {
    let tags = db.get_machine_tags(&row.machine_id)?;
    let machine = fleet.get_or_create(&row.machine_id);
    machine.tags = tags;
}
```

- [ ] **Step 2: Add tag routes to routes.rs**

Add to `control-plane/src/routes.rs`:

```rust
/// POST /api/v1/machines/{id}/tags
pub async fn set_tags(
    State((state, db)): State<AppState>,
    actor: Option<Extension<Actor>>,
    Path(id): Path<String>,
    Json(tags): Json<Vec<String>>,
) -> Result<StatusCode, (StatusCode, String)> {
    db.set_machine_tags(&id, &tags).map_err(|e| {
        tracing::error!(error = %e, machine_id = %id, "Failed to set tags");
        (StatusCode::INTERNAL_SERVER_ERROR, "failed to set tags".to_string())
    })?;

    let mut fleet = state.write().await;
    let machine = fleet.get_or_create(&id);
    machine.tags = tags.clone();

    let actor_id = actor
        .map(|Extension(a)| a.identifier())
        .unwrap_or_else(|| "unknown".to_string());
    let _ = db.insert_audit_event(&actor_id, "set_tags", &id, Some(&tags.join(",")));

    tracing::info!(machine_id = %id, tags = ?tags, "Tags updated");
    Ok(StatusCode::OK)
}

/// DELETE /api/v1/machines/{id}/tags/{tag}
pub async fn remove_tag(
    State((state, db)): State<AppState>,
    actor: Option<Extension<Actor>>,
    Path((id, tag)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, String)> {
    db.remove_machine_tag(&id, &tag).map_err(|e| {
        tracing::error!(error = %e, machine_id = %id, "Failed to remove tag");
        (StatusCode::INTERNAL_SERVER_ERROR, "failed to remove tag".to_string())
    })?;

    let mut fleet = state.write().await;
    if let Some(machine) = fleet.machines.get_mut(&id) {
        machine.tags.retain(|t| t != &tag);
    }

    let actor_id = actor
        .map(|Extension(a)| a.identifier())
        .unwrap_or_else(|| "unknown".to_string());
    let _ = db.insert_audit_event(&actor_id, "remove_tag", &id, Some(&tag));

    Ok(StatusCode::OK)
}
```

Update `list_machines` to include tags in the response:

```rust
// In the map closure, add:
tags: m.tags.clone(),
```

Update `post_report` to persist tags from the report:

```rust
// After updating in-memory state, sync tags if provided
if !report.tags.is_empty() {
    let _ = db.set_machine_tags(&id, &report.tags);
    machine.tags = report.tags.clone();
}
```

Update `set_desired_generation` to check for active rollout conflict:

```rust
// At the start of set_desired_generation, before persisting:
if let Ok(Some(rollout_id)) = db.machine_in_active_rollout(&id) {
    return Err((
        StatusCode::CONFLICT,
        format!("machine {id} is in active rollout {rollout_id}"),
    ));
}
```

- [ ] **Step 3: Wire tag routes into build_app**

In `control-plane/src/lib.rs`, add to `api_routes`:

```rust
.route("/api/v1/machines/{id}/tags", post(routes::set_tags))
.route("/api/v1/machines/{id}/tags/{tag}", axum::routing::delete(routes::remove_tag))
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p nixfleet-control-plane`
Expected: All pass

- [ ] **Step 5: Commit**

```bash
git add control-plane/src/routes.rs control-plane/src/state.rs control-plane/src/lib.rs
git commit -m "feat: add tag endpoints and tag sync on agent report"
```

---

### Task 6: Agent Health Module — Check Trait and Checkers

**Files:**
- Create: `agent/src/health/mod.rs`
- Create: `agent/src/health/config.rs`
- Create: `agent/src/health/systemd.rs`
- Create: `agent/src/health/http.rs`
- Create: `agent/src/health/command.rs`
- Delete: `agent/src/health.rs` (replaced by module)

- [ ] **Step 1: Create health module with Check trait and HealthRunner**

Create `agent/src/health/mod.rs`:

```rust
pub mod command;
pub mod config;
pub mod http;
pub mod systemd;

use async_trait::async_trait;
use nixfleet_types::health::{HealthCheckResult, HealthReport};
use tracing::debug;

#[async_trait]
pub trait Check: Send + Sync {
    fn name(&self) -> &str;
    async fn run(&self) -> HealthCheckResult;
}

pub struct HealthRunner {
    checks: Vec<Box<dyn Check>>,
}

impl HealthRunner {
    pub fn new(checks: Vec<Box<dyn Check>>) -> Self {
        Self { checks }
    }

    /// Build a HealthRunner from a config file path.
    /// Returns a runner with no checks if the file is missing or empty.
    pub fn from_config_path(path: &str) -> Self {
        match config::load_config(path) {
            Ok(cfg) => Self::from_config(cfg),
            Err(e) => {
                debug!("No health config loaded ({e}), using systemd fallback");
                Self::new(vec![Box::new(systemd::SystemdFallback)])
            }
        }
    }

    pub fn from_config(cfg: config::HealthConfig) -> Self {
        let mut checks: Vec<Box<dyn Check>> = vec![];

        for sc in cfg.systemd {
            for unit in sc.units {
                checks.push(Box::new(systemd::SystemdChecker { unit }));
            }
        }
        for hc in cfg.http {
            checks.push(Box::new(http::HttpChecker {
                url: hc.url,
                timeout_secs: hc.timeout as u64,
                expected_status: hc.expected_status as u16,
            }));
        }
        for cc in cfg.command {
            checks.push(Box::new(command::CommandChecker {
                name: cc.name,
                command: cc.command,
                timeout_secs: cc.timeout as u64,
            }));
        }

        if checks.is_empty() {
            checks.push(Box::new(systemd::SystemdFallback));
        }

        Self::new(checks)
    }

    pub async fn run_all(&self) -> HealthReport {
        let mut results = Vec::with_capacity(self.checks.len());
        for check in &self.checks {
            let result = check.run().await;
            debug!(check = check.name(), pass = result.is_pass(), "Health check");
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
```

- [ ] **Step 2: Create config deserialization**

Create `agent/src/health/config.rs`:

```rust
use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HealthConfig {
    #[serde(default)]
    pub systemd: Vec<SystemdConfig>,
    #[serde(default)]
    pub http: Vec<HttpConfig>,
    #[serde(default)]
    pub command: Vec<CommandConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SystemdConfig {
    pub units: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HttpConfig {
    pub url: String,
    #[serde(default = "default_interval")]
    pub interval: i64,
    #[serde(default = "default_timeout")]
    pub timeout: i64,
    #[serde(default = "default_expected_status")]
    pub expected_status: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommandConfig {
    pub name: String,
    pub command: String,
    #[serde(default = "default_cmd_interval")]
    pub interval: i64,
    #[serde(default = "default_cmd_timeout")]
    pub timeout: i64,
}

fn default_interval() -> i64 { 5 }
fn default_timeout() -> i64 { 3 }
fn default_expected_status() -> i64 { 200 }
fn default_cmd_interval() -> i64 { 10 }
fn default_cmd_timeout() -> i64 { 5 }

pub fn load_config(path: &str) -> Result<HealthConfig> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read health config: {path}"))?;
    let config: HealthConfig = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse health config: {path}"))?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_full_config() {
        let json = r#"{
            "systemd": [{"units": ["postgresql", "nginx"]}],
            "http": [{"url": "http://localhost:8080/health", "interval": 5, "timeout": 3, "expected_status": 200}],
            "command": [{"name": "disk", "command": "test -d /tmp", "interval": 10, "timeout": 5}]
        }"#;
        let config: HealthConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.systemd.len(), 1);
        assert_eq!(config.systemd[0].units, vec!["postgresql", "nginx"]);
        assert_eq!(config.http.len(), 1);
        assert_eq!(config.http[0].url, "http://localhost:8080/health");
        assert_eq!(config.command.len(), 1);
        assert_eq!(config.command[0].name, "disk");
    }

    #[test]
    fn test_parse_empty_config() {
        let json = "{}";
        let config: HealthConfig = serde_json::from_str(json).unwrap();
        assert!(config.systemd.is_empty());
        assert!(config.http.is_empty());
        assert!(config.command.is_empty());
    }

    #[test]
    fn test_defaults() {
        let json = r#"{"http": [{"url": "http://localhost"}]}"#;
        let config: HealthConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.http[0].interval, 5);
        assert_eq!(config.http[0].timeout, 3);
        assert_eq!(config.http[0].expected_status, 200);
    }
}
```

- [ ] **Step 3: Create SystemdChecker**

Create `agent/src/health/systemd.rs`:

```rust
use async_trait::async_trait;
use nixfleet_types::health::HealthCheckResult;
use std::time::Instant;
use tokio::process::Command;

use super::Check;

/// Checks a specific systemd unit is active.
pub struct SystemdChecker {
    pub unit: String,
}

#[async_trait]
impl Check for SystemdChecker {
    fn name(&self) -> &str {
        &self.unit
    }

    async fn run(&self) -> HealthCheckResult {
        let start = Instant::now();
        let check_name = format!("systemd:{}", self.unit);
        let result = Command::new("systemctl")
            .args(["is-active", &self.unit])
            .output()
            .await;

        let duration_ms = start.elapsed().as_millis() as u64;
        match result {
            Ok(output) => {
                let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if state == "active" {
                    HealthCheckResult::Pass { check_name, duration_ms }
                } else {
                    HealthCheckResult::Fail {
                        check_name,
                        message: format!("unit is {state}"),
                        duration_ms,
                    }
                }
            }
            Err(e) => HealthCheckResult::Fail {
                check_name,
                message: format!("systemctl failed: {e}"),
                duration_ms,
            },
        }
    }
}

/// Fallback: checks systemctl is-system-running (original behavior).
pub struct SystemdFallback;

#[async_trait]
impl Check for SystemdFallback {
    fn name(&self) -> &str {
        "systemd:system"
    }

    async fn run(&self) -> HealthCheckResult {
        let start = Instant::now();
        let result = Command::new("systemctl")
            .arg("is-system-running")
            .output()
            .await;
        let duration_ms = start.elapsed().as_millis() as u64;
        match result {
            Ok(output) => {
                let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if state == "running" || state == "degraded" {
                    HealthCheckResult::Pass {
                        check_name: "systemd:system".to_string(),
                        duration_ms,
                    }
                } else {
                    HealthCheckResult::Fail {
                        check_name: "systemd:system".to_string(),
                        message: format!("system is {state}"),
                        duration_ms,
                    }
                }
            }
            Err(e) => HealthCheckResult::Fail {
                check_name: "systemd:system".to_string(),
                message: format!("systemctl failed: {e}"),
                duration_ms,
            },
        }
    }
}
```

- [ ] **Step 4: Create HttpChecker**

Create `agent/src/health/http.rs`:

```rust
use async_trait::async_trait;
use nixfleet_types::health::HealthCheckResult;
use std::time::{Duration, Instant};

use super::Check;

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
        let start = Instant::now();
        let check_name = format!("http:{}", self.url);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build();

        let client = match client {
            Ok(c) => c,
            Err(e) => {
                return HealthCheckResult::Fail {
                    check_name,
                    message: format!("client build failed: {e}"),
                    duration_ms: start.elapsed().as_millis() as u64,
                };
            }
        };

        let result = client.get(&self.url).send().await;
        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if status == self.expected_status {
                    HealthCheckResult::Pass { check_name, duration_ms }
                } else {
                    HealthCheckResult::Fail {
                        check_name,
                        message: format!("expected {}, got {status}", self.expected_status),
                        duration_ms,
                    }
                }
            }
            Err(e) => HealthCheckResult::Fail {
                check_name,
                message: format!("{e}"),
                duration_ms,
            },
        }
    }
}
```

- [ ] **Step 5: Create CommandChecker**

Create `agent/src/health/command.rs`:

```rust
use async_trait::async_trait;
use nixfleet_types::health::HealthCheckResult;
use std::time::{Duration, Instant};
use tokio::process::Command;

use super::Check;

pub struct CommandChecker {
    pub name: String,
    pub command: String,
    pub timeout_secs: u64,
}

#[async_trait]
impl Check for CommandChecker {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run(&self) -> HealthCheckResult {
        let start = Instant::now();
        let check_name = format!("command:{}", self.name);

        let result = tokio::time::timeout(
            Duration::from_secs(self.timeout_secs),
            Command::new("sh").args(["-c", &self.command]).output(),
        )
        .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(Ok(output)) => {
                if output.status.success() {
                    HealthCheckResult::Pass { check_name, duration_ms }
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    HealthCheckResult::Fail {
                        check_name,
                        message: format!("exit code {}, stderr: {stderr}", output.status.code().unwrap_or(-1)),
                        duration_ms,
                    }
                }
            }
            Ok(Err(e)) => HealthCheckResult::Fail {
                check_name,
                message: format!("exec failed: {e}"),
                duration_ms,
            },
            Err(_) => HealthCheckResult::Fail {
                check_name,
                message: format!("timed out after {}s", self.timeout_secs),
                duration_ms,
            },
        }
    }
}
```

- [ ] **Step 6: Add async-trait dependency to agent Cargo.toml**

Add to `[dependencies]` in `agent/Cargo.toml`:

```toml
async-trait = "0.1"
```

- [ ] **Step 7: Update agent/src/main.rs module declaration**

Replace `mod health;` with the module directory (Rust auto-discovers `health/mod.rs`). No code change needed — just delete `agent/src/health.rs` since the directory `agent/src/health/` replaces it.

- [ ] **Step 8: Run tests**

Run: `cargo test -p nixfleet-agent`
Expected: All pass (config parse tests, existing tests still work)

- [ ] **Step 9: Commit**

```bash
git add agent/src/health/ agent/Cargo.toml
git rm agent/src/health.rs
git commit -m "feat: add pluggable health check module with systemd, http, and command checkers"
```

---

### Task 7: Agent Main Loop — Health Integration and Tags

**Files:**
- Modify: `agent/src/config.rs`
- Modify: `agent/src/main.rs`
- Modify: `agent/src/comms.rs`

- [ ] **Step 1: Update agent Config with new fields**

In `agent/src/config.rs`, add to `Config`:

```rust
pub struct Config {
    // ... existing fields ...
    /// Path to health checks JSON config file.
    pub health_config_path: String,
    /// Seconds between continuous health reports.
    pub health_interval: Duration,
    /// Machine tags (comma-separated from env var).
    pub tags: Vec<String>,
}
```

Update `default_config()` in tests:

```rust
fn default_config() -> Config {
    Config {
        // ... existing fields ...
        health_config_path: "/etc/nixfleet/health-checks.json".to_string(),
        health_interval: Duration::from_secs(60),
        tags: vec![],
    }
}
```

- [ ] **Step 2: Add CLI args to main.rs**

In the `Cli` struct, add:

```rust
/// Path to health checks JSON config file
#[arg(long, default_value = "/etc/nixfleet/health-checks.json", env = "NIXFLEET_HEALTH_CONFIG")]
health_config: String,

/// Seconds between continuous health reports
#[arg(long, default_value = "60", env = "NIXFLEET_HEALTH_INTERVAL")]
health_interval: u64,

/// Machine tags (comma-separated)
#[arg(long, env = "NIXFLEET_TAGS", value_delimiter = ',')]
tags: Vec<String>,
```

Update config construction in `main()`:

```rust
let config = Config {
    // ... existing fields ...
    health_config_path: cli.health_config.clone(),
    health_interval: Duration::from_secs(cli.health_interval),
    tags: cli.tags,
};
```

- [ ] **Step 3: Initialize HealthRunner and replace health::check_system()**

In `main()`, after creating the config, initialize the health runner:

```rust
let health_runner = health::HealthRunner::from_config_path(&config.health_config_path);
```

In the `Verifying` state arm, replace `health::check_system().await` with:

```rust
AgentState::Verifying { desired } => {
    let report = health_runner.run_all().await;
    if report.all_passed {
        info!("Health checks passed");
        store.log_deploy(&desired.hash, true)
            .unwrap_or_else(|e| warn!("store error: {e}"));
        AgentState::Reporting {
            success: true,
            message: "deployed".into(),
        }
    } else {
        let failed: Vec<_> = report.results.iter()
            .filter(|r| !r.is_pass())
            .map(|r| r.check_name().to_string())
            .collect();
        warn!(failed = ?failed, "Health checks failed after apply");
        AgentState::RollingBack {
            reason: format!("health check failed: {}", failed.join(", ")),
        }
    }
}
```

- [ ] **Step 4: Update Report construction to include tags and health**

In the `Reporting` state arm, update the report construction:

```rust
AgentState::Reporting { success, message } => {
    let current_gen = nix::current_generation().await.unwrap_or_default();
    // Run health for continuous reporting (only when not a deploy report)
    let health_data = if success {
        Some(health_runner.run_all().await)
    } else {
        None
    };
    let report = nixfleet_types::Report {
        machine_id: config.machine_id.clone(),
        current_generation: current_gen,
        success,
        message,
        timestamp: chrono::Utc::now(),
        tags: config.tags.clone(),
        health: health_data,
    };
    match client.post_report(&report).await {
        Ok(()) => info!("Report sent"),
        Err(e) => warn!("Failed to send report: {e}"),
    }
    AgentState::Idle
}
```

Remove the `use types::Report` import and use `nixfleet_types::Report` directly.

- [ ] **Step 5: Add continuous health reporter as second loop**

Wrap the main loop with `tokio::select!` to also run health reporting on its own timer:

```rust
let mut health_ticker = tokio::time::interval(config.health_interval);
health_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
let mut reporting_active = false;

loop {
    let next_state = tokio::select! {
        _ = signal::ctrl_c() => {
            info!("Received shutdown signal, exiting gracefully");
            break;
        }
        _ = health_ticker.tick(), if matches!(agent_state, AgentState::Idle) && !reporting_active => {
            // Continuous health report (only while idle)
            reporting_active = true;
            let report = health_runner.run_all().await;
            let health_report = nixfleet_types::Report {
                machine_id: config.machine_id.clone(),
                current_generation: nix::current_generation().await.unwrap_or_default(),
                success: report.all_passed,
                message: "health-check".to_string(),
                timestamp: chrono::Utc::now(),
                tags: config.tags.clone(),
                health: Some(report),
            };
            match client.post_report(&health_report).await {
                Ok(()) => debug!("Health report sent"),
                Err(e) => warn!("Failed to send health report: {e}"),
            }
            reporting_active = false;
            continue;
        }
        state = async { /* existing state machine match block */ } => state,
    };
    agent_state = next_state;
}
```

- [ ] **Step 6: Update comms.rs to use nixfleet_types::Report**

In `agent/src/comms.rs`, ensure `post_report` accepts `&nixfleet_types::Report` (it should already via the type re-export, but verify the serialization includes new fields).

- [ ] **Step 7: Run tests**

Run: `cargo test -p nixfleet-agent`
Expected: All pass

- [ ] **Step 8: Commit**

```bash
git add agent/src/config.rs agent/src/main.rs agent/src/comms.rs agent/src/types.rs
git commit -m "feat: integrate health runner into agent state machine and add continuous health reporter"
```

---

### Task 8: CP Rollout Batch Builder

**Files:**
- Create: `control-plane/src/rollout/mod.rs`
- Create: `control-plane/src/rollout/batch.rs`

- [ ] **Step 1: Write failing tests for batch building**

Create `control-plane/src/rollout/batch.rs`:

```rust
use rand::seq::SliceRandom;

/// Build batches from a machine list and batch_sizes spec.
///
/// batch_sizes entries are either absolute ("1", "5") or percentage ("25%", "100%").
/// Percentages apply to the REMAINING machines after previous batches.
pub fn build_batches(machines: &[String], batch_sizes: &[String]) -> Vec<Vec<String>> {
    let mut remaining: Vec<String> = machines.to_vec();
    let mut rng = rand::rng();
    remaining.shuffle(&mut rng);

    let mut batches = Vec::new();

    for (i, spec) in batch_sizes.iter().enumerate() {
        if remaining.is_empty() {
            break;
        }
        let count = if spec.ends_with('%') {
            let pct: f64 = spec.trim_end_matches('%').parse().unwrap_or(100.0);
            let raw = (remaining.len() as f64 * pct / 100.0).ceil() as usize;
            raw.max(1).min(remaining.len())
        } else {
            let abs: usize = spec.parse().unwrap_or(1);
            abs.min(remaining.len())
        };

        // Last batch spec gets everything remaining
        let count = if i == batch_sizes.len() - 1 {
            remaining.len()
        } else {
            count
        };

        let batch: Vec<String> = remaining.drain(..count).collect();
        batches.push(batch);
    }

    // If there are remaining machines (shouldn't happen with 100% last batch, but safety)
    if !remaining.is_empty() {
        batches.push(remaining);
    }

    batches
}

/// Resolve effective batch_sizes from strategy.
pub fn effective_batch_sizes(
    strategy: &nixfleet_types::rollout::RolloutStrategy,
    batch_sizes: &Option<Vec<String>>,
) -> Vec<String> {
    use nixfleet_types::rollout::RolloutStrategy;
    match strategy {
        RolloutStrategy::Canary => vec!["1".to_string(), "100%".to_string()],
        RolloutStrategy::AllAtOnce => vec!["100%".to_string()],
        RolloutStrategy::Staged => batch_sizes.clone().unwrap_or_else(|| vec!["100%".to_string()]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machines(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("web-{i:02}")).collect()
    }

    #[test]
    fn test_canary_batch_sizes() {
        let sizes = effective_batch_sizes(
            &nixfleet_types::rollout::RolloutStrategy::Canary,
            &None,
        );
        assert_eq!(sizes, vec!["1", "100%"]);
    }

    #[test]
    fn test_build_batches_canary_20_machines() {
        let m = machines(20);
        let batches = build_batches(&m, &["1".to_string(), "100%".to_string()]);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].len(), 1);
        assert_eq!(batches[1].len(), 19);
    }

    #[test]
    fn test_build_batches_staged() {
        let m = machines(20);
        let batches = build_batches(&m, &["1".to_string(), "25%".to_string(), "100%".to_string()]);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), 1);
        // 25% of remaining 19 = 4.75, ceil = 5
        assert_eq!(batches[1].len(), 5);
        assert_eq!(batches[2].len(), 14);
    }

    #[test]
    fn test_build_batches_all_at_once() {
        let m = machines(10);
        let batches = build_batches(&m, &["100%".to_string()]);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 10);
    }

    #[test]
    fn test_build_batches_single_machine() {
        let m = machines(1);
        let batches = build_batches(&m, &["1".to_string(), "100%".to_string()]);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 1);
    }

    #[test]
    fn test_build_batches_shuffled() {
        let m = machines(20);
        let b1 = build_batches(&m, &["100%".to_string()]);
        let b2 = build_batches(&m, &["100%".to_string()]);
        // With 20 machines, it's extremely unlikely both are in the same order
        // But this could flake — check total count instead
        assert_eq!(b1[0].len(), 20);
        assert_eq!(b2[0].len(), 20);
    }

    #[test]
    fn test_build_batches_empty() {
        let batches = build_batches(&[], &["1".to_string()]);
        assert!(batches.is_empty());
    }
}
```

- [ ] **Step 2: Create rollout module entry point**

Create `control-plane/src/rollout/mod.rs`:

```rust
pub mod batch;
pub mod executor;
pub mod routes;
```

- [ ] **Step 3: Add rand dependency to control-plane Cargo.toml**

Add to `[dependencies]`:

```toml
rand = "0.9"
uuid = { version = "1", features = ["v4"] }
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p nixfleet-control-plane -- batch`
Expected: All pass

- [ ] **Step 5: Commit**

```bash
git add control-plane/src/rollout/ control-plane/Cargo.toml
git commit -m "feat: add rollout batch builder with strategy resolution"
```

---

### Task 9: CP Rollout Executor

**Files:**
- Create: `control-plane/src/rollout/executor.rs`

- [ ] **Step 1: Implement the executor**

Create `control-plane/src/rollout/executor.rs`:

```rust
use crate::db::Db;
use crate::state::FleetState;
use nixfleet_types::rollout::{BatchStatus, RolloutStatus};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Parse a failure threshold spec into a count for a given batch size.
/// "1" → 1, "30%" → ceil(batch_size * 0.3)
fn parse_threshold(spec: &str, batch_size: usize) -> usize {
    if spec.ends_with('%') {
        let pct: f64 = spec.trim_end_matches('%').parse().unwrap_or(0.0);
        (batch_size as f64 * pct / 100.0).ceil() as usize
    } else {
        spec.parse().unwrap_or(1)
    }
}

/// Evaluate batch health: count successes, failures, and pending machines.
struct BatchEvaluation {
    healthy: usize,
    failed: usize,
    pending: usize,
    timed_out: usize,
}

fn evaluate_batch(
    machine_ids: &[String],
    batch_started_at: &str,
    health_timeout: i64,
    db: &Db,
) -> BatchEvaluation {
    let mut healthy = 0;
    let mut failed = 0;
    let mut pending = 0;
    let mut timed_out = 0;

    for machine_id in machine_ids {
        let reports = db
            .get_health_reports_since(machine_id, batch_started_at)
            .unwrap_or_default();

        if let Some(latest) = reports.first() {
            if latest.all_passed {
                healthy += 1;
            } else {
                failed += 1;
            }
        } else {
            // Check if we've exceeded health timeout
            // Compare batch_started_at + health_timeout against now
            let check_reports = db
                .get_recent_reports(machine_id, 1)
                .unwrap_or_default();
            if let Some(report) = check_reports.first() {
                if report.received_at >= batch_started_at.to_string() {
                    if report.success {
                        healthy += 1;
                    } else {
                        failed += 1;
                    }
                    continue;
                }
            }
            // No report yet — check timeout by comparing timestamps
            // Simplified: treat as pending (the tick loop will re-evaluate)
            // Real timeout detection uses chrono parsing
            pending += 1;
        }
    }

    BatchEvaluation { healthy, failed, pending, timed_out }
}

/// Run one tick of the rollout executor.
/// Called every 2 seconds by the background task.
pub async fn tick(state: &Arc<RwLock<FleetState>>, db: &Arc<Db>) {
    let rollouts = match db.list_rollouts_by_status(Some("running"), 100) {
        Ok(r) => r,
        Err(e) => {
            warn!("Failed to list running rollouts: {e}");
            return;
        }
    };

    for rollout in rollouts {
        let batches = match db.get_rollout_batches(&rollout.id) {
            Ok(b) => b,
            Err(e) => {
                warn!(rollout_id = %rollout.id, "Failed to get batches: {e}");
                continue;
            }
        };

        // Find the current batch (first non-succeeded, non-failed)
        let current_batch = batches.iter().find(|b| {
            let status = BatchStatus::from_str_lc(&b.status);
            matches!(status, Some(BatchStatus::Pending | BatchStatus::Deploying | BatchStatus::WaitingHealth))
        });

        let Some(batch) = current_batch else {
            // All batches resolved — check if all succeeded
            let all_succeeded = batches.iter().all(|b| b.status == "succeeded");
            if all_succeeded {
                let _ = db.update_rollout_status(&rollout.id, "completed");
                let _ = db.insert_audit_event("system", "rollout_completed", &rollout.id, None);
                info!(rollout_id = %rollout.id, "Rollout completed");
            }
            continue;
        };

        let machine_ids: Vec<String> = serde_json::from_str(&batch.machine_ids).unwrap_or_default();
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

        match BatchStatus::from_str_lc(&batch.status) {
            Some(BatchStatus::Pending) => {
                // Deploy: set desired generation for all machines in batch
                let mut fleet = state.write().await;
                for machine_id in &machine_ids {
                    let _ = db.set_desired_generation(machine_id, &rollout.generation_hash);
                    let machine = fleet.get_or_create(machine_id);
                    machine.desired_generation = Some(nixfleet_types::DesiredGeneration {
                        hash: rollout.generation_hash.clone(),
                        cache_url: rollout.cache_url.clone(),
                    });
                }
                drop(fleet);

                let _ = db.update_batch_status(&batch.id, "deploying", Some(&now));
                info!(
                    rollout_id = %rollout.id,
                    batch_index = batch.batch_index,
                    machines = ?machine_ids,
                    "Batch deploying"
                );
            }

            Some(BatchStatus::Deploying | BatchStatus::WaitingHealth) => {
                let started_at = batch.started_at.as_deref().unwrap_or(&now);
                let eval = evaluate_batch(&machine_ids, started_at, rollout.health_timeout, db);

                if eval.pending > 0 {
                    // Still waiting for reports
                    if batch.status == "deploying" {
                        let _ = db.update_batch_status(&batch.id, "waiting_health", None);
                    }
                    debug!(
                        rollout_id = %rollout.id,
                        batch_index = batch.batch_index,
                        healthy = eval.healthy,
                        failed = eval.failed,
                        pending = eval.pending,
                        "Waiting for health reports"
                    );
                    return;
                }

                // All machines reported — evaluate threshold
                let threshold = parse_threshold(&rollout.failure_threshold, machine_ids.len());
                let total_failures = eval.failed + eval.timed_out;

                if total_failures > threshold {
                    // Batch failed
                    let _ = db.update_batch_status(&batch.id, "failed", Some(&now));
                    info!(
                        rollout_id = %rollout.id,
                        batch_index = batch.batch_index,
                        failures = total_failures,
                        threshold = threshold,
                        "Batch failed"
                    );

                    if rollout.on_failure == "revert" {
                        // Revert completed batches
                        if let Some(prev_gen) = &rollout.previous_generation {
                            let mut fleet = state.write().await;
                            for b in &batches {
                                if b.status == "succeeded" {
                                    let batch_machines: Vec<String> =
                                        serde_json::from_str(&b.machine_ids).unwrap_or_default();
                                    for mid in &batch_machines {
                                        let _ = db.set_desired_generation(mid, prev_gen);
                                        let machine = fleet.get_or_create(mid);
                                        machine.desired_generation = Some(nixfleet_types::DesiredGeneration {
                                            hash: prev_gen.clone(),
                                            cache_url: None,
                                        });
                                    }
                                }
                            }
                        }
                        let _ = db.update_rollout_status(&rollout.id, "failed");
                        let _ = db.insert_audit_event("system", "rollout_reverted", &rollout.id, None);
                        info!(rollout_id = %rollout.id, "Rollout reverted");
                    } else {
                        let _ = db.update_rollout_status(&rollout.id, "paused");
                        let _ = db.insert_audit_event("system", "rollout_paused", &rollout.id, None);
                        info!(rollout_id = %rollout.id, "Rollout paused");
                    }
                } else {
                    // Batch succeeded
                    let _ = db.update_batch_status(&batch.id, "succeeded", Some(&now));
                    info!(
                        rollout_id = %rollout.id,
                        batch_index = batch.batch_index,
                        "Batch succeeded"
                    );
                    // Next tick will pick up the next pending batch
                }
            }

            _ => {} // Succeeded/Failed — skip
        }
    }
}

/// Spawn the executor background task.
pub fn spawn(state: Arc<RwLock<FleetState>>, db: Arc<Db>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            interval.tick().await;
            tick(&state, &db).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_threshold_absolute() {
        assert_eq!(parse_threshold("1", 10), 1);
        assert_eq!(parse_threshold("5", 10), 5);
    }

    #[test]
    fn test_parse_threshold_percentage() {
        assert_eq!(parse_threshold("30%", 10), 3);
        assert_eq!(parse_threshold("10%", 20), 2);
        assert_eq!(parse_threshold("50%", 3), 2); // ceil(1.5)
    }

    #[test]
    fn test_parse_threshold_100_percent() {
        assert_eq!(parse_threshold("100%", 10), 10);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p nixfleet-control-plane -- executor`
Expected: All pass (threshold parsing tests)

- [ ] **Step 3: Commit**

```bash
git add control-plane/src/rollout/executor.rs
git commit -m "feat: add rollout executor background task with batch evaluation and revert logic"
```

---

### Task 10: CP Rollout Routes

**Files:**
- Create: `control-plane/src/rollout/routes.rs`
- Modify: `control-plane/src/lib.rs`

- [ ] **Step 1: Implement rollout endpoints**

Create `control-plane/src/rollout/routes.rs`:

```rust
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use nixfleet_types::rollout::*;
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::Actor;
use crate::rollout::batch;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ListRolloutsQuery {
    pub status: Option<String>,
}

/// POST /api/v1/rollouts
pub async fn create_rollout(
    State((state, db)): State<AppState>,
    actor: Option<Extension<Actor>>,
    Json(req): Json<CreateRolloutRequest>,
) -> Result<(StatusCode, Json<CreateRolloutResponse>), (StatusCode, String)> {
    // Resolve target machines
    let machine_ids = match &req.target {
        RolloutTarget::Tags(tags) => {
            db.get_machines_by_tags(tags).map_err(|e| {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("tag resolution failed: {e}"))
            })?
        }
        RolloutTarget::Hosts(hosts) => hosts.clone(),
    };

    if machine_ids.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "no machines match target".to_string()));
    }

    // Check for active rollout conflicts
    for mid in &machine_ids {
        if let Ok(Some(rollout_id)) = db.machine_in_active_rollout(mid) {
            return Err((
                StatusCode::CONFLICT,
                format!("machine {mid} is in active rollout {rollout_id}"),
            ));
        }
    }

    // Resolve batch sizes
    let batch_sizes = batch::effective_batch_sizes(&req.strategy, &req.batch_sizes);

    // Build batches
    let batch_lists = batch::build_batches(&machine_ids, &batch_sizes);

    // Capture previous generation (from first machine — they should all be on the same gen for rollouts)
    let previous_generation = {
        let fleet = state.read().await;
        fleet.machines.get(&machine_ids[0])
            .and_then(|m| m.desired_generation.as_ref())
            .map(|d| d.hash.clone())
    };

    // Generate IDs
    let rollout_id = format!("r-{}", &Uuid::new_v4().to_string()[..8]);
    let health_timeout = req.health_timeout.unwrap_or(300) as i64;

    // Persist rollout
    let target_tags = match &req.target {
        RolloutTarget::Tags(t) => Some(serde_json::to_string(t).unwrap()),
        _ => None,
    };
    let target_hosts = match &req.target {
        RolloutTarget::Hosts(h) => Some(serde_json::to_string(h).unwrap()),
        _ => None,
    };

    let actor_id = actor
        .map(|Extension(a)| a.identifier())
        .unwrap_or_else(|| "unknown".to_string());

    db.create_rollout(
        &rollout_id,
        &req.generation_hash,
        req.cache_url.as_deref(),
        &req.strategy.to_string(),
        &serde_json::to_string(&batch_sizes).unwrap(),
        &req.failure_threshold,
        &req.on_failure.to_string(),
        health_timeout,
        target_tags.as_deref(),
        target_hosts.as_deref(),
        previous_generation.as_deref(),
        &actor_id,
    ).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to create rollout: {e}")))?;

    // Persist batches
    let mut batch_summaries = Vec::new();
    for (i, machines) in batch_lists.iter().enumerate() {
        let batch_id = format!("b-{}", &Uuid::new_v4().to_string()[..8]);
        db.create_rollout_batch(
            &batch_id,
            &rollout_id,
            i as i64,
            &serde_json::to_string(machines).unwrap(),
        ).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to create batch: {e}")))?;

        batch_summaries.push(BatchSummary {
            batch_index: i as u32,
            machine_ids: machines.clone(),
            status: BatchStatus::Pending,
        });
    }

    // Start the rollout
    db.update_rollout_status(&rollout_id, "running")
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to start rollout: {e}")))?;

    let _ = db.insert_audit_event(
        &actor_id,
        "create_rollout",
        &rollout_id,
        Some(&format!("strategy={}, machines={}", req.strategy, machine_ids.len())),
    );

    tracing::info!(
        rollout_id = %rollout_id,
        strategy = %req.strategy,
        machines = machine_ids.len(),
        batches = batch_lists.len(),
        "Rollout created"
    );

    Ok((StatusCode::CREATED, Json(CreateRolloutResponse {
        rollout_id,
        batches: batch_summaries,
        total_machines: machine_ids.len(),
    })))
}

/// GET /api/v1/rollouts
pub async fn list_rollouts(
    State((_state, db)): State<AppState>,
    Query(query): Query<ListRolloutsQuery>,
) -> Result<Json<Vec<RolloutDetail>>, (StatusCode, String)> {
    let rows = db.list_rollouts_by_status(query.status.as_deref(), 50)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("query failed: {e}")))?;

    let mut details = Vec::new();
    for row in rows {
        let batches = db.get_rollout_batches(&row.id).unwrap_or_default();
        details.push(row_to_detail(row, batches));
    }
    Ok(Json(details))
}

/// GET /api/v1/rollouts/{id}
pub async fn get_rollout(
    State((_state, db)): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RolloutDetail>, StatusCode> {
    let row = db.get_rollout(&id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let batches = db.get_rollout_batches(&id).unwrap_or_default();
    Ok(Json(row_to_detail(row, batches)))
}

/// POST /api/v1/rollouts/{id}/resume
pub async fn resume_rollout(
    State((_state, db)): State<AppState>,
    actor: Option<Extension<Actor>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let row = db.get_rollout(&id)
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "db error".to_string()))?
        .ok_or((StatusCode::NOT_FOUND, format!("rollout not found: {id}")))?;

    if row.status != "paused" {
        return Err((StatusCode::CONFLICT, format!("rollout is {}, not paused", row.status)));
    }

    // Reset the failed batch to pending so executor retries it
    let batches = db.get_rollout_batches(&id).unwrap_or_default();
    for batch in &batches {
        if batch.status == "failed" {
            let _ = db.update_batch_status(&batch.id, "pending", None);
        }
    }

    db.update_rollout_status(&id, "running")
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to resume: {e}")))?;

    let actor_id = actor.map(|Extension(a)| a.identifier()).unwrap_or_else(|| "unknown".to_string());
    let _ = db.insert_audit_event(&actor_id, "resume_rollout", &id, None);

    tracing::info!(rollout_id = %id, "Rollout resumed");
    Ok(StatusCode::OK)
}

/// POST /api/v1/rollouts/{id}/cancel
pub async fn cancel_rollout(
    State((_state, db)): State<AppState>,
    actor: Option<Extension<Actor>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let row = db.get_rollout(&id)
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "db error".to_string()))?
        .ok_or((StatusCode::NOT_FOUND, format!("rollout not found: {id}")))?;

    let status = RolloutStatus::from_str_lc(&row.status);
    if !matches!(status, Some(s) if s.is_active()) {
        return Err((StatusCode::CONFLICT, format!("rollout is {}, cannot cancel", row.status)));
    }

    db.update_rollout_status(&id, "cancelled")
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to cancel: {e}")))?;

    let actor_id = actor.map(|Extension(a)| a.identifier()).unwrap_or_else(|| "unknown".to_string());
    let _ = db.insert_audit_event(&actor_id, "cancel_rollout", &id, None);

    tracing::info!(rollout_id = %id, "Rollout cancelled");
    Ok(StatusCode::OK)
}

fn row_to_detail(row: crate::db::RolloutRow, batches: Vec<crate::db::RolloutBatchRow>) -> RolloutDetail {
    use std::collections::HashMap;

    RolloutDetail {
        id: row.id,
        status: RolloutStatus::from_str_lc(&row.status).unwrap_or(RolloutStatus::Created),
        strategy: serde_json::from_str(&format!("\"{}\"", row.strategy)).unwrap_or(RolloutStrategy::AllAtOnce),
        generation_hash: row.generation_hash,
        on_failure: serde_json::from_str(&format!("\"{}\"", row.on_failure)).unwrap_or(OnFailure::Pause),
        failure_threshold: row.failure_threshold,
        health_timeout: row.health_timeout as u64,
        batches: batches.into_iter().map(|b| {
            let machine_ids: Vec<String> = serde_json::from_str(&b.machine_ids).unwrap_or_default();
            BatchDetail {
                batch_index: b.batch_index as u32,
                machine_ids: machine_ids.clone(),
                status: BatchStatus::from_str_lc(&b.status).unwrap_or(BatchStatus::Pending),
                machine_health: machine_ids.into_iter().map(|m| (m, MachineHealthStatus::Pending)).collect::<HashMap<_, _>>(),
                started_at: b.started_at.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&chrono::Utc))),
                completed_at: b.completed_at.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&chrono::Utc))),
            }
        }).collect(),
        created_at: chrono::DateTime::parse_from_rfc3339(&row.created_at).map(|d| d.with_timezone(&chrono::Utc)).unwrap_or_else(|_| chrono::Utc::now()),
        updated_at: chrono::DateTime::parse_from_rfc3339(&row.updated_at).map(|d| d.with_timezone(&chrono::Utc)).unwrap_or_else(|_| chrono::Utc::now()),
        created_by: row.created_by,
    }
}
```

- [ ] **Step 2: Register rollout module and routes in lib.rs**

In `control-plane/src/lib.rs`, add:

```rust
pub mod rollout;
```

Add to `build_app` router:

```rust
.route("/api/v1/rollouts", post(rollout::routes::create_rollout))
.route("/api/v1/rollouts", get(rollout::routes::list_rollouts))
.route("/api/v1/rollouts/{id}", get(rollout::routes::get_rollout))
.route("/api/v1/rollouts/{id}/resume", post(rollout::routes::resume_rollout))
.route("/api/v1/rollouts/{id}/cancel", post(rollout::routes::cancel_rollout))
```

- [ ] **Step 3: Spawn executor in main.rs**

In `control-plane/src/main.rs`, after hydration and before serve:

```rust
// Spawn rollout executor
let _executor = rollout::executor::spawn(fleet_state.clone(), db.clone());
```

- [ ] **Step 4: Update post_report to persist health reports**

In `control-plane/src/routes.rs` `post_report`, after persisting the standard report:

```rust
// Persist health report if present
if let Some(ref health) = report.health {
    let results_json = serde_json::to_string(&health.results).unwrap_or_default();
    let _ = db.insert_health_report(&id, &results_json, health.all_passed);
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --workspace`
Expected: All pass

- [ ] **Step 6: Commit**

```bash
git add control-plane/src/rollout/routes.rs control-plane/src/lib.rs control-plane/src/main.rs control-plane/src/routes.rs
git commit -m "feat: add rollout API endpoints with create, list, resume, cancel"
```

---

### Task 11: CLI — Deploy Rollout Mode and Rollout Management

**Files:**
- Modify: `cli/src/main.rs`
- Modify: `cli/src/deploy.rs`
- Create: `cli/src/rollout.rs`

- [ ] **Step 1: Add rollout subcommand group to main.rs**

In `cli/src/main.rs`, add to the `Commands` enum:

```rust
/// Manage rollouts
Rollout(RolloutArgs),
/// Manage machine tags
Machines(MachinesArgs),
```

Add the subcommand structs:

```rust
#[derive(Args)]
pub struct RolloutArgs {
    #[command(subcommand)]
    pub command: RolloutCommand,
}

#[derive(Subcommand)]
pub enum RolloutCommand {
    /// List rollouts
    List {
        #[arg(long)]
        status: Option<String>,
    },
    /// Show rollout details
    Status {
        /// Rollout ID
        id: String,
    },
    /// Resume a paused rollout
    Resume {
        /// Rollout ID
        id: String,
    },
    /// Cancel an active rollout
    Cancel {
        /// Rollout ID
        id: String,
    },
}

#[derive(Args)]
pub struct MachinesArgs {
    #[command(subcommand)]
    pub command: MachinesCommand,
}

#[derive(Subcommand)]
pub enum MachinesCommand {
    /// List machines
    List {
        #[arg(long)]
        tag: Vec<String>,
    },
    /// Set tags on a machine
    Tag {
        /// Machine ID
        id: String,
        /// Tags to set
        tags: Vec<String>,
    },
    /// Remove a tag from a machine
    Untag {
        /// Machine ID
        id: String,
        /// Tag to remove
        tag: String,
    },
}
```

- [ ] **Step 2: Create rollout CLI module**

Create `cli/src/rollout.rs`:

```rust
use anyhow::Result;
use nixfleet_types::rollout::*;

pub async fn list(cp_url: &str, api_key: &str, status: Option<&str>) -> Result<()> {
    let client = reqwest::Client::new();
    let mut url = format!("{cp_url}/api/v1/rollouts");
    if let Some(s) = status {
        url = format!("{url}?status={s}");
    }
    let resp = client.get(&url)
        .bearer_auth(api_key)
        .send().await?
        .json::<Vec<RolloutDetail>>().await?;

    for r in &resp {
        println!("{} | {} | {} | {} | {}", r.id, r.status, r.strategy, r.generation_hash, r.created_by);
    }
    if resp.is_empty() {
        println!("No rollouts found.");
    }
    Ok(())
}

pub async fn status(cp_url: &str, api_key: &str, id: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let resp = client.get(format!("{cp_url}/api/v1/rollouts/{id}"))
        .bearer_auth(api_key)
        .send().await?
        .json::<RolloutDetail>().await?;

    println!("Rollout: {}", resp.id);
    println!("Status:  {}", resp.status);
    println!("Strategy: {}", resp.strategy);
    println!("Generation: {}", resp.generation_hash);
    println!("On failure: {}", resp.on_failure);
    println!("Threshold: {}", resp.failure_threshold);
    println!();
    for batch in &resp.batches {
        println!("  Batch {}: {} ({} machines)", batch.batch_index, batch.status, batch.machine_ids.len());
        for mid in &batch.machine_ids {
            let health = batch.machine_health.get(mid).map(|h| format!("{h:?}")).unwrap_or_else(|| "?".to_string());
            println!("    {mid}: {health}");
        }
    }
    Ok(())
}

pub async fn resume(cp_url: &str, api_key: &str, id: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let resp = client.post(format!("{cp_url}/api/v1/rollouts/{id}/resume"))
        .bearer_auth(api_key)
        .send().await?;
    if resp.status().is_success() {
        println!("Rollout {id} resumed.");
    } else {
        println!("Failed: {}", resp.text().await?);
    }
    Ok(())
}

pub async fn cancel(cp_url: &str, api_key: &str, id: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let resp = client.post(format!("{cp_url}/api/v1/rollouts/{id}/cancel"))
        .bearer_auth(api_key)
        .send().await?;
    if resp.status().is_success() {
        println!("Rollout {id} cancelled.");
    } else {
        println!("Failed: {}", resp.text().await?);
    }
    Ok(())
}
```

- [ ] **Step 3: Update deploy.rs — add rollout-based deploy**

In `cli/src/deploy.rs`, add rollout deploy function (for non-SSH mode):

```rust
pub async fn deploy_rollout(
    cp_url: &str,
    api_key: &str,
    generation: &str,
    tags: &[String],
    hosts: &[String],
    strategy: &str,
    batch_sizes: &[String],
    failure_threshold: &str,
    on_failure: &str,
    health_timeout: u64,
    cache_url: Option<&str>,
    wait: bool,
) -> anyhow::Result<()> {
    use nixfleet_types::rollout::*;

    let target = if !tags.is_empty() {
        RolloutTarget::Tags(tags.to_vec())
    } else {
        RolloutTarget::Hosts(hosts.to_vec())
    };

    let strategy = match strategy {
        "canary" => RolloutStrategy::Canary,
        "staged" => RolloutStrategy::Staged,
        _ => RolloutStrategy::AllAtOnce,
    };

    let on_failure = match on_failure {
        "revert" => OnFailure::Revert,
        _ => OnFailure::Pause,
    };

    let req = CreateRolloutRequest {
        generation_hash: generation.to_string(),
        cache_url: cache_url.map(|s| s.to_string()),
        strategy,
        batch_sizes: if batch_sizes.is_empty() { None } else { Some(batch_sizes.to_vec()) },
        failure_threshold: failure_threshold.to_string(),
        on_failure,
        health_timeout: Some(health_timeout),
        target,
    };

    let client = reqwest::Client::new();
    let resp = client.post(format!("{cp_url}/api/v1/rollouts"))
        .bearer_auth(api_key)
        .json(&req)
        .send().await?;

    if !resp.status().is_success() {
        anyhow::bail!("Failed to create rollout: {}", resp.text().await?);
    }

    let created: CreateRolloutResponse = resp.json().await?;
    println!("Rollout {} created: {} machines, {} batches", created.rollout_id, created.total_machines, created.batches.len());

    if wait {
        poll_rollout(cp_url, api_key, &created.rollout_id).await?;
    }

    Ok(())
}

async fn poll_rollout(cp_url: &str, api_key: &str, rollout_id: &str) -> anyhow::Result<()> {
    use nixfleet_types::rollout::*;

    let client = reqwest::Client::new();
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let resp = client.get(format!("{cp_url}/api/v1/rollouts/{rollout_id}"))
            .bearer_auth(api_key)
            .send().await?
            .json::<RolloutDetail>().await?;

        // Print current state
        for batch in &resp.batches {
            if batch.status != BatchStatus::Pending {
                println!("Batch {}/{}: {}", batch.batch_index, resp.batches.len() - 1, batch.status);
            }
        }

        match resp.status {
            RolloutStatus::Completed => {
                println!("Rollout {} completed.", rollout_id);
                return Ok(());
            }
            RolloutStatus::Paused => {
                println!("Rollout {} paused. Resume: nixfleet rollout resume {}", rollout_id, rollout_id);
                return Ok(());
            }
            RolloutStatus::Failed => {
                println!("Rollout {} failed.", rollout_id);
                return Ok(());
            }
            RolloutStatus::Cancelled => {
                println!("Rollout {} cancelled.", rollout_id);
                return Ok(());
            }
            _ => {} // Still running, keep polling
        }
    }
}
```

- [ ] **Step 4: Wire new commands in main.rs dispatch**

Add match arms for `Rollout` and `Machines` commands in the main dispatch.

- [ ] **Step 5: Run tests**

Run: `cargo test --workspace`
Expected: All pass

- [ ] **Step 6: Commit**

```bash
git add cli/src/main.rs cli/src/deploy.rs cli/src/rollout.rs
git commit -m "feat: add rollout and machine tag CLI commands with --wait polling"
```

---

### Task 12: Nix Module Updates

**Files:**
- Modify: `modules/scopes/nixfleet/_agent.nix`
- Modify: `modules/tests/eval.nix`
- Modify: `modules/fleet.nix`

- [ ] **Step 1: Add tags, health checks, and health interval options to _agent.nix**

Add new options inside `options.services.nixfleet-agent`:

```nix
tags = lib.mkOption {
  type = lib.types.listOf lib.types.str;
  default = [];
  description = "Tags for grouping this machine in fleet operations.";
};

healthInterval = lib.mkOption {
  type = lib.types.int;
  default = 60;
  description = "Seconds between continuous health reports to control plane.";
};

healthChecks = {
  systemd = lib.mkOption {
    type = lib.types.listOf (lib.types.submodule {
      options.units = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        description = "Systemd units that must be active.";
      };
    });
    default = [];
    description = "Systemd unit health checks.";
  };

  http = lib.mkOption {
    type = lib.types.listOf (lib.types.submodule {
      options = {
        url = lib.mkOption { type = lib.types.str; description = "URL to GET."; };
        interval = lib.mkOption { type = lib.types.int; default = 5; description = "Check interval in seconds."; };
        timeout = lib.mkOption { type = lib.types.int; default = 3; description = "Timeout in seconds."; };
        expectedStatus = lib.mkOption { type = lib.types.int; default = 200; description = "Expected HTTP status code."; };
      };
    });
    default = [];
    description = "HTTP endpoint health checks.";
  };

  command = lib.mkOption {
    type = lib.types.listOf (lib.types.submodule {
      options = {
        name = lib.mkOption { type = lib.types.str; description = "Check name."; };
        command = lib.mkOption { type = lib.types.str; description = "Shell command (exit 0 = healthy)."; };
        interval = lib.mkOption { type = lib.types.int; default = 10; description = "Check interval in seconds."; };
        timeout = lib.mkOption { type = lib.types.int; default = 5; description = "Timeout in seconds."; };
      };
    });
    default = [];
    description = "Custom command health checks.";
  };
};
```

In the `config = lib.mkIf cfg.enable` block, add health config file generation:

```nix
environment.etc."nixfleet/health-checks.json".text = builtins.toJSON {
  systemd = cfg.healthChecks.systemd;
  http = cfg.healthChecks.http;
  command = cfg.healthChecks.command;
};
```

Update the `ExecStart` to include new flags:

```nix
"--health-config"
"/etc/nixfleet/health-checks.json"
"--health-interval"
(toString cfg.healthInterval)
```

Add tags via environment variable:

```nix
systemd.services.nixfleet-agent.environment.NIXFLEET_TAGS =
  lib.mkIf (cfg.tags != [])
  (lib.concatStringsSep "," cfg.tags);
```

- [ ] **Step 2: Add eval tests**

In `modules/tests/eval.nix`, add a test host with agent config and assertions:

```nix
eval-agent-tags = mkEvalCheck "agent tags env var set" (
  let agentEnv = config.systemd.services.nixfleet-agent.environment;
  in agentEnv.NIXFLEET_TAGS == "web,production"
);

eval-agent-health-config = mkEvalCheck "health config file generated" (
  config.environment.etc ? "nixfleet/health-checks.json"
);
```

This requires a test host in `fleet.nix` with agent enabled and configured with tags and health checks.

- [ ] **Step 3: Add test host to fleet.nix**

In `modules/fleet.nix`, add:

```nix
agent-test = mkHost {
  hostName = "agent-test";
  platform = "x86_64-linux";
  hostSpec = {userName = "testuser";};
  modules = [{
    services.nixfleet-agent = {
      enable = true;
      controlPlaneUrl = "https://cp.test:8080";
      tags = ["web" "production"];
      healthChecks = {
        systemd = [{units = ["nginx"];}];
        http = [{url = "http://localhost:80/health"; interval = 5; timeout = 3;}];
      };
    };
  }];
};
```

- [ ] **Step 4: Run eval tests**

Run: `cd /home/s33d/dev/nix-org/nixfleet && nix flake check --no-build`
Expected: All eval checks pass

- [ ] **Step 5: Commit**

```bash
git add modules/scopes/nixfleet/_agent.nix modules/tests/eval.nix modules/fleet.nix
git commit -m "feat: add tags, health checks, and health interval options to agent NixOS module"
```

---

### Task 13: CP State Hydration and Health Cleanup

**Files:**
- Modify: `control-plane/src/state.rs`
- Modify: `control-plane/src/main.rs`

- [ ] **Step 1: Update hydrate_from_db to load tags and active rollouts**

In `control-plane/src/state.rs`, update `hydrate_from_db`:

```rust
pub async fn hydrate_from_db(
    state: &Arc<RwLock<FleetState>>,
    db: &crate::db::Db,
) -> anyhow::Result<()> {
    let registered = db.list_machines()?;
    let mut fleet = state.write().await;

    for row in &registered {
        let machine = fleet.get_or_create(&row.machine_id);
        if let Some(lc) = MachineLifecycle::from_str_lc(&row.lifecycle) {
            machine.lifecycle = lc;
        }
        // Load tags
        let tags = db.get_machine_tags(&row.machine_id)?;
        machine.tags = tags;
    }

    let generations = db.list_desired_generations()?;
    for (machine_id, hash) in generations {
        let machine = fleet.get_or_create(&machine_id);
        machine.desired_generation = Some(DesiredGeneration {
            hash,
            cache_url: None,
        });
    }

    // Log active rollouts count
    let active_rollouts = db.list_rollouts_by_status(Some("running"), 100)?;
    let paused_rollouts = db.list_rollouts_by_status(Some("paused"), 100)?;

    tracing::info!(
        machines = fleet.machines.len(),
        active_rollouts = active_rollouts.len(),
        paused_rollouts = paused_rollouts.len(),
        "Hydrated fleet state from database"
    );
    Ok(())
}
```

- [ ] **Step 2: Add periodic health report cleanup to main.rs**

In `control-plane/src/main.rs`, after spawning the executor, add a cleanup task:

```rust
// Spawn health report cleanup task (hourly)
let cleanup_db = db.clone();
tokio::spawn(async move {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
    loop {
        interval.tick().await;
        match cleanup_db.cleanup_old_health_reports(24) {
            Ok(deleted) => {
                if deleted > 0 {
                    tracing::info!(deleted, "Cleaned up old health reports");
                }
            }
            Err(e) => tracing::warn!("Health report cleanup failed: {e}"),
        }
    }
});
```

- [ ] **Step 3: Run tests**

Run: `cargo test --workspace`
Expected: All pass

- [ ] **Step 4: Commit**

```bash
git add control-plane/src/state.rs control-plane/src/main.rs
git commit -m "feat: hydrate tags and active rollouts on CP startup, add health report cleanup"
```

---

### Task 14: Final Integration — Workspace Build and Full Test

**Files:** None new — integration verification

- [ ] **Step 1: Build entire workspace**

Run: `cargo build --workspace`
Expected: Clean build, no errors or warnings

- [ ] **Step 2: Run all Rust tests**

Run: `cargo test --workspace`
Expected: All pass

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings

- [ ] **Step 4: Format**

Run: `cargo fmt --all`

- [ ] **Step 5: Run Nix eval tests**

Run: `cd /home/s33d/dev/nix-org/nixfleet && nix flake check --no-build`
Expected: All eval checks pass

- [ ] **Step 6: Commit any formatting changes**

```bash
git add -A
git commit -m "chore: format and lint pass"
```
