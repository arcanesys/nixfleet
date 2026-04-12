//! Shared test harness for CP scenario tests.
//!
//! Declared as a sibling module in `scenarios.rs` and imported by each
//! scenario file via `use super::harness`. The harness spins up a real
//! `build_app` router on a random port backed by a fresh temp SQLite DB.
//! Helpers exist to seed API keys, create releases, create rollouts, and
//! submit fake agent reports. No mocks of the subject under test.

#![allow(dead_code)] // each test file uses a subset; allow unused helpers

use metrics_exporter_prometheus::PrometheusBuilder;
use nixfleet_control_plane::{build_app, db, state};
use nixfleet_types::release::{CreateReleaseRequest, CreateReleaseResponse, ReleaseEntry};
use nixfleet_types::rollout::{
    CreateRolloutRequest, CreateRolloutResponse, OnFailure, RolloutDetail, RolloutStatus,
    RolloutStrategy, RolloutTarget,
};
use nixfleet_types::Report;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::RwLock;

pub const TEST_API_KEY: &str = "test-admin-key";
pub const TEST_READONLY_KEY: &str = "test-readonly-key";
pub const TEST_DEPLOY_KEY: &str = "test-deploy-key";

/// Everything a scenario needs to drive the CP.
pub struct Cp {
    pub base: String,
    pub admin: reqwest::Client,
    pub db: Arc<db::Db>,
    pub fleet: Arc<RwLock<state::FleetState>>,
    pub tempdir: tempfile::TempDir,
    pub db_path: String,
}

/// Spawn a fresh CP. New temp DB, new random port, new in-memory fleet state.
pub async fn spawn_cp() -> Cp {
    spawn_cp_at(None).await
}

/// Spawn a CP reusing an existing db path (for the F6 hydration scenario).
///
/// If `path` is `None`, a fresh temp directory is created and the new
/// `Cp` owns it via its `tempdir` field.
///
/// If `path` is `Some`, the caller is responsible for keeping the owning
/// `TempDir` alive for the full duration of the second spawn. The typical
/// F6 pattern is:
///
/// ```ignore
/// let cp1 = spawn_cp().await;
/// let db_path = cp1.db_path.clone();
/// // ... do things with cp1 ...
/// let cp2 = spawn_cp_at(Some(&db_path)).await;
/// // cp1 must remain in scope until cp2 is done so cp1.tempdir is not dropped.
/// ```
///
/// The `tempdir` field on the returned `Cp` in this branch is a fresh
/// unused temp directory — it exists only to keep the `Cp` shape uniform
/// and is not load-bearing for the reused db path.
pub async fn spawn_cp_at(path: Option<&str>) -> Cp {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let db_path = match path {
        Some(p) => p.to_string(),
        None => tempdir
            .path()
            .join("state.db")
            .to_string_lossy()
            .into_owned(),
    };

    let database = Arc::new(db::Db::new(&db_path).expect("db::new"));
    database.migrate().expect("db::migrate");

    // Seed three keys so auth scenarios can drive all three tiers. Skip
    // seeding when the DB already has keys (e.g. F6 hydration scenario
    // respawns a CP against an existing on-disk DB).
    if !database.has_api_keys().expect("has_api_keys") {
        seed_key(&database, TEST_API_KEY, "integration-admin", "admin");
        seed_key(&database, TEST_DEPLOY_KEY, "integration-deploy", "deploy");
        seed_key(
            &database,
            TEST_READONLY_KEY,
            "integration-readonly",
            "readonly",
        );
    }

    let fleet = Arc::new(RwLock::new(state::FleetState::new()));

    // The Prometheus recorder is process-global: only the first
    // install wins. Store the handle in a OnceLock so every
    // subsequent spawn_cp reuses the *installed* recorder's handle
    // (not a freshly-built dead one). Without this, metrics tests
    // fail in the merged single-binary because a different test's
    // spawn_cp wins the global install and later calls get handles
    // to recorders that were never installed.
    static METRICS_HANDLE: std::sync::OnceLock<Arc<metrics_exporter_prometheus::PrometheusHandle>> =
        std::sync::OnceLock::new();
    let handle = METRICS_HANDLE
        .get_or_init(|| {
            let recorder = PrometheusBuilder::new().build_recorder();
            let h = Arc::new(recorder.handle());
            metrics::set_global_recorder(recorder).ok();
            h
        })
        .clone();

    let app = build_app(fleet.clone(), database.clone(), handle);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "authorization",
        reqwest::header::HeaderValue::from_str(&format!("Bearer {}", TEST_API_KEY)).unwrap(),
    );
    let admin = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap();

    Cp {
        base: format!("http://{addr}"),
        admin,
        db: database,
        fleet,
        tempdir,
        db_path,
    }
}

/// A client authenticated with an arbitrary key.
pub fn client_with_key(raw_key: &str) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "authorization",
        reqwest::header::HeaderValue::from_str(&format!("Bearer {raw_key}")).unwrap(),
    );
    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap()
}

/// A client with no Authorization header at all (for 401 negative tests).
pub fn client_anonymous() -> reqwest::Client {
    reqwest::Client::new()
}

fn seed_key(db: &db::Db, raw: &str, name: &str, role: &str) {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    let key_hash = hex::encode(hasher.finalize());
    db.insert_api_key(&key_hash, name, role).unwrap();
}

/// Register a machine in the CP fleet (bypasses the HTTP layer for setup speed).
pub async fn register_machine(cp: &Cp, machine_id: &str, tags: &[&str]) {
    cp.db.register_machine(machine_id, "active").unwrap();
    let tags_owned: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
    cp.db.set_machine_tags(machine_id, &tags_owned).unwrap();
    let mut fleet = cp.fleet.write().await;
    fleet.get_or_create(machine_id);
}

/// Create a release via `POST /api/v1/releases` with the given entries.
pub async fn create_release(cp: &Cp, hosts: &[(&str, &str)]) -> String {
    let entries: Vec<ReleaseEntry> = hosts
        .iter()
        .map(|(hostname, store_path)| ReleaseEntry {
            hostname: (*hostname).to_string(),
            store_path: (*store_path).to_string(),
            platform: "x86_64-linux".to_string(),
            tags: vec![],
        })
        .collect();
    let body = CreateReleaseRequest {
        flake_ref: Some("test".to_string()),
        flake_rev: Some("deadbeef".to_string()),
        cache_url: None,
        entries,
    };
    let resp = cp
        .admin
        .post(format!("{}/api/v1/releases", cp.base))
        .json(&body)
        .send()
        .await
        .expect("create_release request");
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    assert_eq!(status, 201, "create_release failed: {text}");
    let created: CreateReleaseResponse = serde_json::from_str(&text).unwrap();
    created.id
}

/// Create a rollout targeting the given tag, referencing an existing release.
#[allow(clippy::too_many_arguments)]
pub async fn create_rollout_for_tag(
    cp: &Cp,
    release_id: &str,
    tag: &str,
    strategy: RolloutStrategy,
    batch_sizes: Option<Vec<&str>>,
    failure_threshold: &str,
    on_failure: OnFailure,
    health_timeout: u64,
) -> String {
    let body = CreateRolloutRequest {
        release_id: release_id.to_string(),
        cache_url: None,
        strategy,
        batch_sizes: batch_sizes.map(|v| v.into_iter().map(|s| s.to_string()).collect()),
        failure_threshold: failure_threshold.to_string(),
        on_failure,
        health_timeout: Some(health_timeout),
        target: RolloutTarget::Tags(vec![tag.to_string()]),
    };
    let resp = cp
        .admin
        .post(format!("{}/api/v1/rollouts", cp.base))
        .json(&body)
        .send()
        .await
        .expect("create_rollout request");
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    assert_eq!(status, 201, "create_rollout failed: {text}");
    let created: CreateRolloutResponse = serde_json::from_str(&text).unwrap();
    created.rollout_id
}

/// Submit a fake `POST /api/v1/machines/{id}/report` on behalf of an agent.
pub async fn fake_agent_report(
    cp: &Cp,
    machine_id: &str,
    current_generation: &str,
    success: bool,
    message: &str,
    tags: &[&str],
) {
    let report = Report {
        machine_id: machine_id.to_string(),
        current_generation: current_generation.to_string(),
        success,
        message: message.to_string(),
        timestamp: chrono::Utc::now(),
        tags: tags.iter().map(|s| s.to_string()).collect(),
        health: None,
    };
    let resp = cp
        .admin
        .post(format!("{}/api/v1/machines/{}/report", cp.base, machine_id))
        .json(&report)
        .send()
        .await
        .expect("report request");
    let status = resp.status();
    assert!(
        status.is_success(),
        "report failed ({status}): {}",
        resp.text().await.unwrap_or_default()
    );
}

/// Poll a rollout's `status` field until it matches `want` or the deadline elapses.
pub async fn wait_rollout_status(
    cp: &Cp,
    rollout_id: &str,
    want: RolloutStatus,
    within: std::time::Duration,
) -> RolloutDetail {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let resp = cp
            .admin
            .get(format!("{}/api/v1/rollouts/{}", cp.base, rollout_id))
            .send()
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "GET rollout {rollout_id} failed with HTTP {}",
            resp.status()
        );
        let body = resp.text().await.expect("read rollout body");
        let detail: RolloutDetail = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("decode RolloutDetail from {body:?}: {e}"));
        if detail.status == want {
            return detail;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "rollout {rollout_id} did not reach {want:?} within {within:?}; last status = {:?}",
                detail.status
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Run a single executor tick synchronously.
pub async fn tick_once(cp: &Cp) {
    nixfleet_control_plane::rollout::executor::test_support::tick_for_tests(&cp.fleet, &cp.db)
        .await
        .expect("tick_for_tests");
}

// =====================================================================
// High-level convenience helpers
//
// The three helpers below compose the lower-level primitives above
// into the patterns that ~30 scenario tests repeat verbatim. They
// exist purely to keep the tests readable — each caller saves
// ~10 lines of fixture setup.
// =====================================================================

/// Spin up a fresh CP with the canonical "1 machine, 1 release entry,
/// 1 all-at-once rollout, zero tolerance, pause on failure" fixture.
/// Returns `(Cp, release_id, rollout_id)`.
///
/// Canonical defaults: `machine_id = "web-01"`, `tag = "web"`,
/// `strategy = AllAtOnce`, `failure_threshold = "0"`,
/// `on_failure = Pause`, `health_timeout = 60`. The caller supplies
/// `store_path` so it can be referenced later in `fake_agent_report`
/// or `insert_health_report` calls that have to match the release
/// entry (generation gate).
///
/// Tests that need a non-canonical strategy, threshold, or multi-
/// machine fleet still compose the lower-level `register_machine` /
/// `create_release` / `create_rollout_for_tag` primitives directly.
pub async fn spawn_cp_with_rollout(store_path: &str) -> (Cp, String, String) {
    let cp = spawn_cp().await;
    register_machine(&cp, "web-01", &["web"]).await;
    let release_id = create_release(&cp, &[("web-01", store_path)]).await;
    let rollout_id = create_rollout_for_tag(
        &cp,
        &release_id,
        "web",
        RolloutStrategy::AllAtOnce,
        None,
        "0",
        OnFailure::Pause,
        60,
    )
    .await;
    (cp, release_id, rollout_id)
}

/// Paired report: submits both a `fake_agent_report` (visible to the
/// executor's generation gate + failure_count) AND an
/// `insert_health_report` (visible to the batch evaluator's health
/// gate). The executor checks both; almost every failure/recovery
/// scenario needs both to arrive together.
///
/// `healthy = true` → success=true, message="ok", health_results="{}".
/// `healthy = false` → success=false, message="boom",
/// health_results="{\"fail\":true}".
pub async fn agent_reports_health(cp: &Cp, machine_id: &str, store_path: &str, healthy: bool) {
    let (message, health_body) = if healthy {
        ("ok", "{}")
    } else {
        ("boom", "{\"fail\":true}")
    };
    fake_agent_report(cp, machine_id, store_path, healthy, message, &["web"]).await;
    cp.db
        .insert_health_report(machine_id, health_body, healthy)
        .unwrap();
}

/// Send an HTTP request via `builder` and assert the response status
/// equals `expected`. Used by `route_coverage.rs` to collapse every
/// "let resp = ...; .send().await; assert_eq!(resp.status(), N)"
/// triple into a single line:
///
/// ```ignore
/// assert_status(cp.admin.get(format!("{}/...", cp.base)), 200).await;
/// ```
pub async fn assert_status(builder: reqwest::RequestBuilder, expected: u16) {
    let resp = builder.send().await.expect("request");
    assert_eq!(
        resp.status().as_u16(),
        expected,
        "unexpected status: body = {}",
        resp.text().await.unwrap_or_default()
    );
}
