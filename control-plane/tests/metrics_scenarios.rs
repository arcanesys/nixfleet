//! ME1, ME2 — Prometheus metrics scenarios.
//!
//! Spec Section 4. Verify that `/metrics` exposes the expected metric
//! names after real traffic, and that the HTTP middleware correctly
//! counts per normalized path.
//!
//! ## Process-global state quirks
//!
//! The `metrics_exporter_prometheus` recorder is installed exactly
//! once per test binary via an `OnceLock` in the harness. Only the
//! handle returned alongside the recorder that actually got installed
//! (the first `spawn_cp` call in the binary) can render metrics; any
//! later `spawn_cp` builds a new un-installed recorder and returns a
//! dead handle that renders an empty body.
//!
//! On top of that, `#[tokio::test]` creates a **fresh runtime per
//! test**, so any task spawned from one test (including the axum
//! server inside `harness::spawn_cp`) is cancelled as soon as that
//! test's runtime is dropped. A `tokio::sync::OnceCell<Cp>` shared
//! across tests is therefore not enough by itself — the server would
//! die between tests.
//!
//! This file solves both problems by owning its own long-lived
//! background tokio runtime on a dedicated thread. The shared `Cp`
//! lives on that runtime and stays up for the entire binary. Both
//! `me1` and `me2` are declared as plain `#[test]` functions that
//! `block_on` work on the shared runtime via `Handle`, which keeps
//! all async state (server task, db handles, http clients) on a
//! single runtime whose lifetime outlives every test.
//!
//! Assertions use `>= N` (never `== N`) so any cross-test pollution
//! from the shared `Cp` cannot cause false failures.

#[path = "harness.rs"]
mod harness;

use nixfleet_types::metrics as m;
use nixfleet_types::rollout::{OnFailure, RolloutStatus, RolloutStrategy};
use std::future::Future;
use std::sync::{Mutex, MutexGuard, OnceLock};
use tokio::runtime::{Handle, Runtime};

/// Handle to a dedicated multi-threaded tokio runtime that lives on
/// its own OS thread for the entire binary. `Runtime` is leaked to
/// guarantee it outlives every test.
fn shared_runtime() -> &'static Handle {
    static RUNTIME: OnceLock<Handle> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        let rt = Runtime::new().expect("build shared tokio runtime");
        let handle = rt.handle().clone();
        // Leak the runtime so it (and all tasks spawned on it) live
        // for the lifetime of the process.
        Box::leak(Box::new(rt));
        handle
    })
}

/// Shared `Cp` instance for all metrics scenarios in this binary.
/// Lives on the shared runtime.
fn shared_cp() -> &'static harness::Cp {
    static CP: OnceLock<harness::Cp> = OnceLock::new();
    CP.get_or_init(|| shared_runtime().block_on(async { harness::spawn_cp().await }))
}

/// Serializes test execution so ME1 and ME2 do not race on the
/// shared counter values or db state.
fn metrics_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Drive an async test body against the shared `Cp`. Holds the test
/// lock for the duration, blocks on the shared runtime so every
/// spawned task (including the axum server) survives across tests.
fn run_metrics_test<F, Fut>(f: F)
where
    F: FnOnce(&'static harness::Cp) -> Fut,
    Fut: Future<Output = ()>,
{
    let _guard: MutexGuard<'_, ()> = metrics_test_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let cp = shared_cp();
    shared_runtime().block_on(f(cp));
}

async fn scrape(cp: &harness::Cp) -> String {
    cp.admin
        .get(format!("{}/metrics", cp.base))
        .send()
        .await
        .expect("scrape /metrics")
        .text()
        .await
        .expect("read /metrics body")
}

/// ME1 — after creating + completing a rollout, every CP-side declared
/// metric appears in `/metrics`, the `nixfleet_rollouts_total` counter
/// has recorded the completion, and no archived Phase 2 metrics leak.
///
/// Scope note: `shared/src/metrics.rs` declares 7 CP-side constants and 6
/// agent-side constants. The CP process never emits the agent-side
/// constants (they are produced by the `nixfleet-agent` binary), so this
/// test only asserts presence of the 7 CP-side names. Asserting the
/// agent names would be a category error.
#[test]
fn me1_metrics_populated_after_rollout_cycle() {
    run_metrics_test(|cp| async move {
        harness::register_machine(cp, "web-01", &["web"]).await;
        let release_id = harness::create_release(cp, &[("web-01", "/nix/store/me1-web-01")]).await;
        let rollout_id = harness::create_rollout_for_tag(
            cp,
            &release_id,
            "web",
            RolloutStrategy::AllAtOnce,
            None,
            "0",
            OnFailure::Pause,
            60,
        )
        .await;

        // Drive the full cycle: pending → deploying → waiting_health → completed.
        harness::tick_once(cp).await;
        harness::agent_reports_health(cp, "web-01", "/nix/store/me1-web-01", true).await;
        harness::tick_once(cp).await;
        harness::tick_once(cp).await;

        let detail = harness::wait_rollout_status(
            cp,
            &rollout_id,
            RolloutStatus::Completed,
            std::time::Duration::from_secs(2),
        )
        .await;
        assert!(
            matches!(detail.status, RolloutStatus::Completed),
            "rollout must reach Completed before scraping metrics"
        );

        let body = scrape(cp).await;

        // Positive: every CP-side declared metric name must be present.
        // Source of truth: shared/src/metrics.rs (the `m::*` constants below
        // come from that module, so renaming there will refuse to compile
        // here rather than silently skipping).
        let cp_metrics = [
            m::FLEET_SIZE,
            m::MACHINES_BY_LIFECYCLE,
            m::MACHINE_LAST_SEEN_TIMESTAMP,
            m::HTTP_REQUESTS_TOTAL,
            m::HTTP_REQUEST_DURATION_SECONDS,
            m::ROLLOUTS_ACTIVE,
            m::ROLLOUTS_TOTAL,
        ];
        for metric in cp_metrics {
            assert!(
                body.contains(metric),
                "/metrics missing expected metric '{metric}'\n---\n{body}\n---"
            );
        }

        // Positive: nixfleet_rollouts_total{status="completed"} must be >= 1.
        // Use the LAST matching line in case cross-test pollution produced
        // multiple samples (it should not, but be safe).
        let completed_line = body
            .lines()
            .rev()
            .find(|l| {
                !l.starts_with('#')
                    && l.contains("nixfleet_rollouts_total")
                    && l.contains("status=\"completed\"")
            })
            .expect("no nixfleet_rollouts_total{status=\"completed\"} sample line");
        let value: f64 = completed_line
            .rsplit(' ')
            .next()
            .expect("no value on completed line")
            .parse()
            .expect("parse completed counter value");
        assert!(
        value >= 1.0,
        "nixfleet_rollouts_total{{status=\"completed\"}} must be >= 1, got {value} (line: {completed_line})"
    );

        // Negative: archived Phase 2 metric names must NOT appear anywhere
        // in the scrape. These were removed when policy/schedule features
        // were deleted in the Phase 2 hardening squash.
        for gone in ["nixfleet_policy", "nixfleet_schedule"] {
            assert!(
                !body.contains(gone),
                "removed metric name '{gone}' leaked into /metrics"
            );
        }
    });
}

/// ME2 — the HTTP metrics middleware emits a `nixfleet_http_requests_total`
/// counter keyed by normalized path, and requests to paths we never call
/// do not appear.
///
/// Delta note: because the shared `Cp` may have served other HTTP
/// calls before this test acquires the lock (ME1 hits
/// `/api/v1/rollouts`, `/api/v1/machines/{id}/report`, etc. but not
/// `/api/v1/machines` itself), we take a baseline scrape, drive 3
/// GETs, and assert the summed counter grew by >= 3.
#[test]
fn me2_http_middleware_counts_requests() {
    run_metrics_test(|cp| async move {
        let before = scrape(cp).await;
        let before_value = parse_machines_counter(&before).unwrap_or(0.0);

        // Drive 3 GET /api/v1/machines calls.
        for _ in 0..3 {
            let resp = cp
                .admin
                .get(format!("{}/api/v1/machines", cp.base))
                .send()
                .await
                .expect("GET /api/v1/machines");
            assert!(
                resp.status().is_success(),
                "GET /api/v1/machines failed with {}",
                resp.status()
            );
        }

        let body = scrape(cp).await;

        // Positive: find a counter line for path="/api/v1/machines" and
        // assert its value grew by >= 3 relative to the pre-traffic baseline.
        let after_value = parse_machines_counter(&body)
            .expect("no nixfleet_http_requests_total sample for /api/v1/machines after 3 GETs");
        let delta = after_value - before_value;
        assert!(
        delta >= 3.0,
        "nixfleet_http_requests_total for /api/v1/machines must grow by >= 3, got delta={delta} (before={before_value}, after={after_value})"
    );

        // Negative: a path we never called must not have a counter line.
        let leaked = body.lines().any(|l| {
            !l.starts_with('#')
                && l.contains("nixfleet_http_requests_total")
                && l.contains("path=\"/never\"")
        });
        assert!(
        !leaked,
        "nixfleet_http_requests_total has a sample for path=\"/never\" but that path was never called"
    );
    });
}

/// Return the summed `nixfleet_http_requests_total` counter value for
/// `path="/api/v1/machines"` across all label combinations (method,
/// status, etc.). Returns `None` if no such sample is present at all.
///
/// We sum (rather than max) so that 3 consecutive calls always grow
/// the total by exactly 3 regardless of how label combinations split.
fn parse_machines_counter(body: &str) -> Option<f64> {
    let mut total = 0.0;
    let mut seen = false;
    for line in body.lines() {
        if line.starts_with('#')
            || !line.contains("nixfleet_http_requests_total")
            || !line.contains("path=\"/api/v1/machines\"")
        {
            continue;
        }
        if let Some(value_str) = line.rsplit(' ').next() {
            if let Ok(value) = value_str.parse::<f64>() {
                total += value;
                seen = true;
            }
        }
    }
    if seen {
        Some(total)
    } else {
        None
    }
}
