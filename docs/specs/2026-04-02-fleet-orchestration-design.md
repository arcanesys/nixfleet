# Fleet Orchestration: Tags, Health Checks & Rollout Strategies

**Date:** 2026-04-02
**Status:** Draft
**Scope:** Machine tags, declarative health checks, staged rollout orchestration

## Overview

This spec covers three tightly coupled features that form the core fleet orchestration loop:

1. **Machine tags** — grouping machines for targeted operations
2. **Declarative health checks** — pluggable health verification on agents
3. **Rollout strategies** — CP-driven staged deployment with automatic pause/revert

Together these make nixfleet a fleet management product rather than a `nixos-rebuild` wrapper. The agent remains simple and autonomous; all orchestration intelligence lives in the control plane.

## Architecture

```
CLI (operator)
  │
  │ POST /api/v1/rollouts (strategy, target, generation)
  ▼
Control Plane
  ├── Rollout executor (background task, 2s tick)
  │     ├── Resolves target (tags → machine set)
  │     ├── Builds batches from strategy
  │     ├── Sets desired_generation per batch
  │     ├── Evaluates health reports per batch
  │     └── Advances / pauses / reverts
  ├── HTTP API (rollout CRUD, machine tags, health data)
  └── SQLite (rollouts, batches, tags, health reports)
         ▲
         │ Reports (deployment + health)
         │
Agents (per machine)
  ├── State machine: Idle → Checking → Fetching → Applying → Verifying → Reporting
  ├── Health runner: systemd + http + command checks
  ├── Continuous health reporter (while Idle)
  └── Local rollback autonomy (if Verifying fails)
```

The agent has zero knowledge of rollouts. It sees desired_generation changes, deploys, runs health checks, reports results. The CP correlates reports to rollout batches by machine_id and timestamp.

---

## 1. Machine Tags

### Data Model

Tags are string labels on machine records. No hierarchy, no namespacing.

```sql
CREATE TABLE machine_tags (
    machine_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    PRIMARY KEY (machine_id, tag),
    FOREIGN KEY (machine_id) REFERENCES machines(machine_id)
);
CREATE INDEX idx_machine_tags_tag ON machine_tags(tag);
```

### Nix Module

New option on `_agent.nix`:

```nix
services.nixfleet-agent.tags = lib.mkOption {
  type = lib.types.listOf lib.types.str;
  default = [];
  description = "Tags for grouping this machine in fleet operations";
};
```

Tags are passed as `NIXFLEET_TAGS` environment variable (comma-separated). Sent to CP on registration and every report to stay in sync with Nix config.

### API Changes

- `POST /api/v1/machines/{id}/register` — body gains `tags: Vec<String>`
- `POST /api/v1/machines/{id}/report` — body gains `tags: Vec<String>`
- `GET /api/v1/machines` — response gains `tags: Vec<String>` per machine
- `GET /api/v1/machines?tag=web&tag=production` — filter by tags (AND logic)
- `POST /api/v1/machines/{id}/tags` — set tags (replaces all). Admin scope.
- `DELETE /api/v1/machines/{id}/tags/{tag}` — remove single tag. Admin scope.

### Shared Types

```rust
pub struct RegisterMachineRequest {
    pub lifecycle: Option<MachineLifecycle>,
    pub tags: Option<Vec<String>>,
}

pub struct Report {
    pub machine_id: String,
    pub current_generation: String,
    pub success: bool,
    pub message: String,
    pub timestamp: DateTime<Utc>,
    pub tags: Vec<String>,
    pub health: Option<HealthReport>,
}

pub struct MachineStatus {
    // ... existing fields ...
    pub tags: Vec<String>,
}
```

### CLI

```
nixfleet machines list --tag production
nixfleet machines tag web-01 production eu-west
nixfleet machines untag web-01 eu-west
```

---

## 2. Declarative Health Checks

### Nix Module

New options on `_agent.nix`:

```nix
services.nixfleet-agent.healthChecks = {
  systemd = lib.mkOption {
    type = lib.types.listOf (lib.types.submodule {
      options = {
        units = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          description = "Systemd units that must be active";
        };
      };
    });
    default = [];
  };

  http = lib.mkOption {
    type = lib.types.listOf (lib.types.submodule {
      options = {
        url = lib.mkOption { type = lib.types.str; };
        interval = lib.mkOption { type = lib.types.int; default = 5; };
        timeout = lib.mkOption { type = lib.types.int; default = 3; };
        expectedStatus = lib.mkOption { type = lib.types.int; default = 200; };
      };
    });
    default = [];
  };

  command = lib.mkOption {
    type = lib.types.listOf (lib.types.submodule {
      options = {
        name = lib.mkOption { type = lib.types.str; };
        command = lib.mkOption { type = lib.types.str; };
        interval = lib.mkOption { type = lib.types.int; default = 10; };
        timeout = lib.mkOption { type = lib.types.int; default = 5; };
      };
    });
    default = [];
  };
};

healthInterval = lib.mkOption {
  type = lib.types.int;
  default = 60;
  description = "Seconds between continuous health reports to control plane";
};
```

### Config File

The Nix module serializes health check definitions to `/etc/nixfleet/health-checks.json`. The agent reads this at startup. No CLI flags for health checks — declarative only.

```json
{
  "systemd": [{"units": ["postgresql", "nginx"]}],
  "http": [{"url": "http://localhost:8080/health", "interval": 5, "timeout": 3, "expected_status": 200}],
  "command": [{"name": "disk-space", "command": "test $(df --output=pcent / | tail -1 | tr -d '% ') -lt 90", "interval": 10, "timeout": 5}]
}
```

### Agent Implementation

New module structure replacing `health.rs`:

```
agent/src/health/
├── mod.rs          — HealthRunner, Check trait, HealthReport
├── config.rs       — deserialize health-checks.json
├── systemd.rs      — SystemdChecker (systemctl is-active per unit)
├── http.rs         — HttpChecker (reqwest GET, check status code)
└── command.rs      — CommandChecker (tokio::process, check exit code)
```

Check trait:

```rust
#[async_trait]
pub trait Check: Send + Sync {
    fn name(&self) -> &str;
    async fn run(&self) -> HealthCheckResult;
}

pub enum HealthCheckResult {
    Pass { check_name: String, duration_ms: u64 },
    Fail { check_name: String, message: String, duration_ms: u64 },
}

pub struct HealthReport {
    pub results: Vec<HealthCheckResult>,
    pub all_passed: bool,
    pub timestamp: DateTime<Utc>,
}
```

Adding new check types (tcp, disk, process) is a new struct implementing `Check` + a new match arm in config deserialization. No architecture changes needed.

### Two Contexts Where Health Runs

1. **Post-deploy verification** (Verifying state) — run all checks once. Replaces current `systemctl is-system-running`. If `all_passed`, proceed to Reporting. If not, local RollingBack.

2. **Continuous health reporting** (while Idle) — every `health_interval` seconds, run all checks, POST report to CP with health results. The CP uses these for rollout batch evaluation.

### Fallback

If no health checks are configured (empty or missing config file), the agent falls back to `systemctl is-system-running`. Zero-config deployments still work.

### Backward Compatibility

`health` is `Option<HealthReport>` on `Report`. Old agents send `None`, CP treats as "no health data" (not unhealthy). Old CPs ignore the new field.

---

## 3. Rollout Orchestration

### Design Decisions

- **Agent retains local rollback autonomy** — if health checks fail post-deploy, the agent rolls back immediately without waiting for CP. This handles the case where the CP is unreachable.
- **CP drives fleet-wide rollout decisions** — using aggregated health data from agents.
- **Strategy defined at deploy time** (path to CP-side per-group defaults later).
- **Default: pause on failure.** Revert available via `--on-failure revert`.
- **Rollout state persisted to SQLite** — survives CP restart.
- **One active rollout per machine** — CP rejects overlapping rollouts (409 Conflict).

### Rollout Lifecycle

```
Created → Running → Completed
                 → Paused → Running (resumed) or Cancelled
                 → Failed
                 → Cancelled
```

### Data Model

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

CREATE TABLE health_reports (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    machine_id TEXT NOT NULL,
    results TEXT NOT NULL,
    all_passed INTEGER NOT NULL,
    received_at TEXT NOT NULL
);
CREATE INDEX idx_health_reports_machine ON health_reports(machine_id, received_at);
```

### Batch Building

1. CLI sends rollout request with target (tags or explicit hosts) + strategy
2. CP resolves target to machine list
3. CP builds batches from `batch_sizes`:
   - `"1"` → first 1 machine (canary)
   - `"25%"` → 25% of remaining machines (rounded up)
   - `"100%"` → all remaining
4. Machine order within batches: shuffled
5. Batches and rollout persisted before any deployment starts

Example: 20 machines, `--batch-size 1,25%,100%`:

```
Batch 0: [web-14]                          — 1 machine (canary)
Batch 1: [web-03, web-19, web-07, web-11, web-02]  — 25% of 19 = 5
Batch 2: [web-01, web-04, ...]             — all remaining 14
```

### Strategy Shorthands

- `Canary`: `batch_sizes` defaults to `["1", "100%"]`
- `AllAtOnce`: `batch_sizes` defaults to `["100%"]`
- `Staged`: `batch_sizes` required

### Rollout Executor

Background tokio task in the CP, ticking every 2 seconds. Separate from HTTP handlers. Shares `FleetState` via `Arc<RwLock<>>`.

Per-tick logic for each Running rollout:

1. **Pending batch** → set `desired_generation` for all machines in batch, status → deploying
2. **Deploying / waiting-health batch** → evaluate batch health:
   - For each machine: check most recent report since `batch.started_at`
   - Classify: Healthy, Unhealthy, RolledBack, StillWaiting, TimedOut
   - If any StillWaiting (within `health_timeout`) → do nothing
   - If all resolved → count failures vs `failure_threshold`
     - Under threshold → batch succeeded, advance to next batch
     - Over threshold → batch failed:
       - `on_failure = pause` → rollout status → Paused
       - `on_failure = revert` → set `previous_generation` on all completed batch machines, rollout status → Failed
3. **Last batch succeeded** → rollout status → Completed

### Health Timeout

If a machine doesn't report within `health_timeout` seconds (default: 300) after its batch starts deploying, it's treated as a failure and counts toward the failure threshold.

### Failure Threshold

Configurable per rollout. Accepts absolute count (`"1"`) or percentage (`"30%"`). Evaluated against batch size.

### Resume Logic

When a paused rollout is resumed: status → Running, the failed batch is retried (machines get `desired_generation` set again). Executor picks it up on next tick.

### Revert Semantics

"Revert" means "set `desired_generation` to `previous_generation` for machines in completed batches." This reuses the existing deployment mechanism — the agent sees a new desired generation and deploys it. No new agent capability needed.

### Graceful Shutdown

On SIGTERM, the executor stops advancing batches. On restart, it hydrates from SQLite and resumes. Batches in `deploying` or `waiting-health` are re-evaluated from stored reports.

### Concurrent Rollout Protection

A machine can only be in one active (Created/Running/Paused) rollout. Creating a rollout targeting a machine already in an active rollout returns 409 Conflict.

`set_desired_generation` (direct single-machine endpoint) also returns 409 if the machine is in an active rollout.

---

## 4. API Endpoints

### New Endpoints

```
POST   /api/v1/rollouts                  — create rollout (deploy/admin)
GET    /api/v1/rollouts                  — list rollouts (readonly/deploy/admin)
GET    /api/v1/rollouts/{id}             — rollout detail (readonly/deploy/admin)
POST   /api/v1/rollouts/{id}/resume      — resume paused (deploy/admin)
POST   /api/v1/rollouts/{id}/cancel      — cancel rollout (deploy/admin)
POST   /api/v1/machines/{id}/tags        — set tags (admin)
DELETE /api/v1/machines/{id}/tags/{tag}   — remove tag (admin)
```

### Request/Response Types

```rust
pub struct CreateRolloutRequest {
    pub generation_hash: String,
    pub cache_url: Option<String>,
    pub strategy: RolloutStrategy,
    pub batch_sizes: Option<Vec<String>>,  // required for Staged, ignored for Canary/AllAtOnce
    pub failure_threshold: String,         // absolute ("1") or percentage ("30%")
    pub on_failure: OnFailure,
    pub health_timeout: Option<u64>,       // seconds, default 300
    pub target: RolloutTarget,
}

pub enum RolloutTarget {
    Tags(Vec<String>),
    Hosts(Vec<String>),
}

pub enum RolloutStrategy { Canary, Staged, AllAtOnce }
pub enum OnFailure { Pause, Revert }

pub struct CreateRolloutResponse {
    pub rollout_id: String,
    pub batches: Vec<BatchSummary>,
    pub total_machines: usize,
}

pub struct BatchSummary {
    pub batch_index: u32,
    pub machine_ids: Vec<String>,
    pub status: BatchStatus,
}

pub struct RolloutDetail {
    pub id: String,
    pub status: RolloutStatus,
    pub strategy: RolloutStrategy,
    pub generation_hash: String,
    pub on_failure: OnFailure,
    pub failure_threshold: String,
    pub batches: Vec<BatchDetail>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: String,
}

pub struct BatchDetail {
    pub batch_index: u32,
    pub machine_ids: Vec<String>,
    pub status: BatchStatus,
    pub machine_health: HashMap<String, MachineHealthStatus>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

pub enum BatchStatus { Pending, Deploying, WaitingHealth, Succeeded, Failed }

pub enum MachineHealthStatus {
    Pending,
    Healthy,
    Unhealthy(String),
    TimedOut,
    RolledBack,
}

pub enum RolloutStatus { Created, Running, Paused, Completed, Failed, Cancelled }
```

### Existing Endpoint Changes

`set_desired_generation` remains for direct single-machine operations. Returns 409 if machine is in an active rollout.

---

## 5. CLI Interface

### Deploy Command

```
nixfleet deploy \
  --generation <store-path-hash> \
  --tag production --tag eu-west \
  --strategy canary \
  --batch-size 1,25%,100% \
  --failure-threshold 1 \
  --on-failure pause \
  --health-timeout 300 \
  --cache-url https://cache.example.com \
  --wait
```

`--wait` streams progress by polling `GET /api/v1/rollouts/{id}` at 2-second intervals. Without `--wait`, deploy returns immediately with rollout ID.

### Rollout Management

```
nixfleet rollout list [--status running]
nixfleet rollout status <id>
nixfleet rollout resume <id>
nixfleet rollout cancel <id>
```

### Machine Management (Updated)

```
nixfleet machines list [--tag production]
nixfleet machines tag <id> tag1 tag2
nixfleet machines untag <id> tag1
```

### SSH Fallback

Existing `deploy --ssh` and `rollback --ssh` remain unchanged. They bypass the CP entirely. Rollout system is CP-only.

---

## 6. Agent Changes Summary

Minimal changes. The agent stays simple.

### New Config

- `--health-config /etc/nixfleet/health-checks.json` (default, set by Nix module)
- `--health-interval 60` (continuous reporting interval, seconds)
- `NIXFLEET_TAGS` environment variable (comma-separated)

### New Module Structure

```
agent/src/health/
├── mod.rs          — HealthRunner, Check trait, HealthReport
├── config.rs       — deserialize health-checks.json
├── systemd.rs      — SystemdChecker
├── http.rs         — HttpChecker
└── command.rs      — CommandChecker
```

### Two Independent Loops

```
Loop 1: State machine (existing, modified Verifying step)
    Verifying now runs HealthRunner instead of systemctl is-system-running

Loop 2: Continuous health reporter (new, runs while Idle)
    Every health_interval: run HealthRunner, POST report with health data
```

### Report Payload

```rust
pub struct Report {
    pub machine_id: String,
    pub current_generation: String,
    pub success: bool,
    pub message: String,
    pub timestamp: DateTime<Utc>,
    pub tags: Vec<String>,
    pub health: Option<HealthReport>,
}
```

Both new fields are backward-compatible: `tags` defaults to empty vec, `health` defaults to None.

---

## 7. Database Migrations

Three new migrations (V4, V5, V6) added to the CP's refinery chain:

- **V4**: `machine_tags` table with composite PK + tag index
- **V5**: `rollouts` table with status index + `rollout_batches` table with rollout_id index
- **V6**: `health_reports` table with (machine_id, received_at) index

### Retention

- `health_reports`: 24 hours per machine. Hourly cleanup task.
- `rollouts` + `rollout_batches`: indefinite (audit trail).
- `machine_tags`: mirrors Nix config, no retention.

### Hydration

On CP startup: existing machine/generation hydration + tags loaded into MachineState + Running/Paused rollouts restored into executor.

---

## 8. Testing Strategy

### Rust Tests

| Area | Tests |
|------|-------|
| Shared types | Serialization round-trips for tags, health, rollout types |
| Health checkers | SystemdChecker, HttpChecker, CommandChecker: pass/fail/timeout |
| HealthRunner | Aggregation, fallback when no config |
| Health config | JSON deserialization, empty/missing file |
| Report | Serialization with health + tags fields |
| DB tags | set/get/filter (AND logic)/remove |
| DB rollouts | create/list/status transitions/batch updates |
| DB health | insert/query since/cleanup retention |
| DB hydration | Rollouts and tags restored on startup |
| Batch building | Absolute counts, percentages, rounding, shuffling |
| Batch evaluation | All healthy, under threshold, over threshold, timed out, still waiting |
| Rollout state machine | All transitions: Created→Running→Completed, Paused→Running, etc. |
| Revert logic | Completed batches get previous_generation |
| Concurrent rollout | 409 on overlapping machines |
| Tag resolution | AND logic, empty result |
| Route handlers | Auth scopes, validation, response shapes |
| CLI parsing | Flag combinations, strategy shorthands, defaults |

### Nix Tests

| Tier | Tests |
|------|-------|
| Eval | Tags env var, health config file, health defaults |
| VM | Agent starts with health config, JSON file exists and valid |

### Out of Scope (Manual)

- Multi-VM rollout end-to-end
- Real `switch-to-configuration` during rollout
- Network partition scenarios (chaos testing — Phase 4)

---

## 9. Future Extensions (Not in This Spec)

- **Additional health check types**: tcp, disk, process — new Check implementations, no architecture change
- **CP-side rollout defaults per tag group** (path to B) — `policies` table, CLI `policy set/list`, deploy resolves policy before creating rollout request. API shape unchanged.
- **Webhooks** on rollout state changes — notify Slack/PagerDuty on pause/failure
- **Dashboard** — read-only web UI consuming existing API endpoints
- **Rollout scheduling** — "deploy at 02:00 UTC" via `created` status with `scheduled_at` field
