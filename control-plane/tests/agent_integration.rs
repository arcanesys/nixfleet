/// Integration tests for the NixFleet agent <-> control plane communication cycle.
///
/// Each test spins up a real Axum server on a random port (via `TcpListener::bind("127.0.0.1:0")`),
/// then drives it with `reqwest` -- the same HTTP semantics any real agent or operator would use.
///
/// Scenarios covered:
///   1. Happy-path deploy: set generation -> agent polls -> agent reports success.
///   2. Failed deploy: agent reports failure; inventory reflects "error" state.
///   3. Multi-machine isolation: three machines do not interfere with each other.
use metrics_exporter_prometheus::PrometheusBuilder;
use nixfleet_control_plane::{build_app, db, state};
use nixfleet_types::{DesiredGeneration, MachineStatus, Report};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::RwLock;

const RAW_TEST_KEY: &str = "test-key";

// ---- Helpers ----------------------------------------------------------------

/// Spawns a real control-plane server on a random port.
///
/// Returns (base_url, authenticated_client, db, fleet_state, TempDir). The server runs on a
/// background Tokio task and is torn down when the test process exits. The `TempDir` must be kept
/// alive for the duration of the test to prevent the SQLite database from being deleted.
async fn spawn_server() -> (
    String,
    reqwest::Client,
    Arc<db::Db>,
    Arc<RwLock<state::FleetState>>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db").to_string_lossy().into_owned();

    let database = Arc::new(db::Db::new(&db_path).expect("db::new"));
    database.migrate().expect("db::migrate");

    // Seed an API key for integration tests.
    let mut hasher = Sha256::new();
    hasher.update(RAW_TEST_KEY.as_bytes());
    let key_hash = hex::encode(hasher.finalize());
    database
        .insert_api_key(&key_hash, "integration-test", "admin")
        .unwrap();

    let fleet_state = Arc::new(RwLock::new(state::FleetState::new()));

    // Use a non-installed recorder so tests don't conflict with a global recorder.
    let recorder = PrometheusBuilder::new().build_recorder();
    let metrics_handle = Arc::new(recorder.handle());
    // Install this recorder as the global for this test process (idempotent via once_cell).
    static METRICS_INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    METRICS_INSTALLED.get_or_init(|| {
        metrics::set_global_recorder(recorder).ok();
    });

    let app = build_app(fleet_state.clone(), database.clone(), metrics_handle);

    // Bind to a random OS-assigned port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind to random port");
    let addr = listener.local_addr().expect("local_addr");

    // Serve in background -- the task will be cancelled when the runtime shuts down.
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Build a client with the default auth header.
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "authorization",
        reqwest::header::HeaderValue::from_static("Bearer test-key"),
    );
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap();

    (format!("http://{addr}"), client, database, fleet_state, dir)
}

/// Set the desired generation for a machine directly via DB + in-memory state.
///
/// This bypasses the removed `set-generation` HTTP endpoint and replicates what
/// the rollout executor does when it processes a batch.
async fn set_desired_gen(
    db: &db::Db,
    fleet_state: &Arc<RwLock<state::FleetState>>,
    machine_id: &str,
    hash: &str,
) {
    db.set_desired_generation(machine_id, hash).unwrap();
    let mut fleet = fleet_state.write().await;
    let machine = fleet.get_or_create(machine_id);
    machine.desired_generation = Some(DesiredGeneration {
        hash: hash.to_string(),
        cache_url: None,
        poll_hint: None,
    });
}

// ---- Auth -------------------------------------------------------------------

#[tokio::test]
async fn test_unauthenticated_request_rejected() {
    let (base, _client, _db, _state, _dir) = spawn_server().await;
    let no_auth_client = reqwest::Client::new();
    let resp = no_auth_client
        .get(format!("{base}/api/v1/machines"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_health_no_auth_required() {
    let (base, _client, _db, _state, _dir) = spawn_server().await;
    let no_auth_client = reqwest::Client::new();
    let resp = no_auth_client
        .get(format!("{base}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

// ---- Health -----------------------------------------------------------------

#[tokio::test]
async fn test_health_endpoint() {
    let (base, _client, _db, _state, _dir) = spawn_server().await;
    let resp = reqwest::get(format!("{base}/health"))
        .await
        .expect("GET /health");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "ok");
}

// ---- Full agent <-> control plane cycle -------------------------------------

/// Happy path: operator sets generation -> agent polls -> agent reports success.
#[tokio::test]
async fn test_agent_control_plane_cycle() {
    let (base, client, db, fleet_state, _dir) = spawn_server().await;
    let machine_id = "test";
    let gen_hash = "/nix/store/abc123zzz-nixos-system-test-25.05";

    // 1. No desired generation yet -- must return 404.
    let resp = client
        .get(format!(
            "{base}/api/v1/machines/{machine_id}/desired-generation"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "unknown machine must 404");

    // 2. Set a desired generation directly (replaces removed set-generation endpoint).
    set_desired_gen(&db, &fleet_state, machine_id, gen_hash).await;

    // 3. Agent polls -- must receive the generation that was just set.
    let get_resp = client
        .get(format!(
            "{base}/api/v1/machines/{machine_id}/desired-generation"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(get_resp.status(), 200);
    let desired: DesiredGeneration = get_resp.json().await.unwrap();
    assert_eq!(desired.hash, gen_hash);
    assert!(desired.cache_url.is_none());

    // 4. Agent reports successful application.
    let report = Report {
        machine_id: machine_id.to_string(),
        current_generation: gen_hash.to_string(),
        success: true,
        message: "generation applied".to_string(),
        timestamp: chrono::Utc::now(),
        tags: vec![],
        health: None,
    };
    let report_resp = client
        .post(format!("{base}/api/v1/machines/{machine_id}/report"))
        .json(&report)
        .send()
        .await
        .unwrap();
    assert_eq!(report_resp.status(), 200, "report must be accepted");

    // 5. Inventory reflects the correct state.
    let list_resp = client
        .get(format!("{base}/api/v1/machines"))
        .send()
        .await
        .unwrap();
    assert_eq!(list_resp.status(), 200);
    let machines: Vec<MachineStatus> = list_resp.json().await.unwrap();

    let machine = machines
        .iter()
        .find(|m| m.machine_id == machine_id)
        .expect("machine must appear in inventory");

    assert_eq!(
        machine.desired_generation.as_deref(),
        Some(gen_hash),
        "desired generation must be persisted"
    );
    assert_eq!(
        machine.current_generation, gen_hash,
        "current generation must reflect the last report"
    );
    assert_eq!(
        machine.system_state, "ok",
        "successful report maps to 'ok' state"
    );
}

// ---- Failure / rollback path ------------------------------------------------

/// Agent applies a generation, health check fails, reports failure + rollback target.
/// Control plane must record the failure and expose "error" state in inventory.
#[tokio::test]
async fn test_failed_deploy_reported_correctly() {
    let (base, client, db, fleet_state, _dir) = spawn_server().await;
    let machine_id = "dev-01";
    let bad_hash = "/nix/store/bad000-nixos-system-dev-01-25.05";
    let rollback_gen = "/nix/store/good111-nixos-system-dev-01-25.05";

    // Set desired generation directly (replaces removed set-generation endpoint).
    set_desired_gen(&db, &fleet_state, machine_id, bad_hash).await;

    // Agent attempts deployment, health check fails, rolls back to previous generation.
    let report = Report {
        machine_id: machine_id.to_string(),
        current_generation: rollback_gen.to_string(),
        success: false,
        message: "rolled back: health check failed after 30s".to_string(),
        timestamp: chrono::Utc::now(),
        tags: vec![],
        health: None,
    };
    client
        .post(format!("{base}/api/v1/machines/{machine_id}/report"))
        .json(&report)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // Inventory reflects the failure.
    let machines: Vec<MachineStatus> = client
        .get(format!("{base}/api/v1/machines"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let machine = machines
        .iter()
        .find(|m| m.machine_id == machine_id)
        .expect("machine must appear in inventory");

    assert_eq!(
        machine.system_state, "error",
        "failed report maps to 'error' state"
    );
    assert_eq!(
        machine.current_generation, rollback_gen,
        "current generation reflects rollback target"
    );
    assert_eq!(
        machine.desired_generation.as_deref(),
        Some(bad_hash),
        "desired generation unchanged -- operator must update it explicitly"
    );
}

// ---- Multi-machine isolation ------------------------------------------------

/// Multiple machines must not interfere with each other's desired generation.
#[tokio::test]
async fn test_multi_machine_isolation() {
    let (base, client, db, fleet_state, _dir) = spawn_server().await;

    let machines = [
        ("web-01", "/nix/store/aaa-nixos-system-web-01"),
        ("dev-01", "/nix/store/bbb-nixos-system-dev-01"),
        ("mac-01", "/nix/store/ccc-nixos-system-mac-01"),
    ];

    // Set desired generations directly for all machines.
    for (id, hash) in &machines {
        set_desired_gen(&db, &fleet_state, id, hash).await;
    }

    // Each machine must see only its own desired generation.
    for (id, hash) in &machines {
        let resp = client
            .get(format!("{base}/api/v1/machines/{id}/desired-generation"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let desired: DesiredGeneration = resp.json().await.unwrap();
        assert_eq!(
            &desired.hash, hash,
            "machine {id} must see its own generation"
        );
    }

    // Inventory must list all three machines.
    let list: Vec<MachineStatus> = client
        .get(format!("{base}/api/v1/machines"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.len(), 3, "all three machines must appear in inventory");
}

// ---- Machine Registry -------------------------------------------------------

/// Pre-registering a machine sets it to Pending lifecycle.
#[tokio::test]
async fn test_register_machine() {
    let (base, client, _db, _state, _dir) = spawn_server().await;

    let resp = client
        .post(format!("{base}/api/v1/machines/new-host/register"))
        .json(&serde_json::json!({ "lifecycle": "pending" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201, "register must return 201 Created");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["machine_id"], "new-host");
    assert_eq!(body["lifecycle"], "pending");

    // Machine should appear in inventory with pending lifecycle.
    let machines: Vec<MachineStatus> = client
        .get(format!("{base}/api/v1/machines"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let machine = machines
        .iter()
        .find(|m| m.machine_id == "new-host")
        .expect("registered machine must appear in inventory");
    assert_eq!(
        machine.lifecycle,
        nixfleet_types::MachineLifecycle::Pending,
        "registered machine must be in pending state"
    );
}

/// Lifecycle transitions: Active -> Maintenance -> Active.
#[tokio::test]
async fn test_lifecycle_transitions() {
    let (base, client, _db, _state, _dir) = spawn_server().await;

    // Register as pending, then auto-activate via report.
    client
        .post(format!("{base}/api/v1/machines/trans-host/register"))
        .json(&serde_json::json!({ "lifecycle": "pending" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // Send a report to auto-activate.
    let report = Report {
        machine_id: "trans-host".to_string(),
        current_generation: "/nix/store/gen1".to_string(),
        success: true,
        message: "deployed".to_string(),
        timestamp: chrono::Utc::now(),
        tags: vec![],
        health: None,
    };
    client
        .post(format!("{base}/api/v1/machines/trans-host/report"))
        .json(&report)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // Verify active.
    let machines: Vec<MachineStatus> = client
        .get(format!("{base}/api/v1/machines"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let machine = machines
        .iter()
        .find(|m| m.machine_id == "trans-host")
        .unwrap();
    assert_eq!(machine.lifecycle, nixfleet_types::MachineLifecycle::Active);

    // Transition Active -> Maintenance.
    let resp = client
        .patch(format!("{base}/api/v1/machines/trans-host/lifecycle"))
        .json(&serde_json::json!({ "lifecycle": "maintenance" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Transition Maintenance -> Active.
    let resp = client
        .patch(format!("{base}/api/v1/machines/trans-host/lifecycle"))
        .json(&serde_json::json!({ "lifecycle": "active" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

/// First agent report auto-transitions Pending -> Active.
#[tokio::test]
async fn test_auto_activate_on_first_report() {
    let (base, client, _db, _state, _dir) = spawn_server().await;

    // Register as pending.
    client
        .post(format!("{base}/api/v1/machines/auto-host/register"))
        .json(&serde_json::json!({ "lifecycle": "pending" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // Verify pending.
    let machines: Vec<MachineStatus> = client
        .get(format!("{base}/api/v1/machines"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let machine = machines
        .iter()
        .find(|m| m.machine_id == "auto-host")
        .unwrap();
    assert_eq!(machine.lifecycle, nixfleet_types::MachineLifecycle::Pending);

    // Agent sends first report.
    let report = Report {
        machine_id: "auto-host".to_string(),
        current_generation: "/nix/store/first-gen".to_string(),
        success: true,
        message: "initial boot".to_string(),
        timestamp: chrono::Utc::now(),
        tags: vec![],
        health: None,
    };
    client
        .post(format!("{base}/api/v1/machines/auto-host/report"))
        .json(&report)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // Verify auto-activated.
    let machines: Vec<MachineStatus> = client
        .get(format!("{base}/api/v1/machines"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let machine = machines
        .iter()
        .find(|m| m.machine_id == "auto-host")
        .unwrap();
    assert_eq!(
        machine.lifecycle,
        nixfleet_types::MachineLifecycle::Active,
        "first report must auto-activate a pending machine"
    );
}

/// Invalid lifecycle transitions must be rejected with 409 Conflict.
#[tokio::test]
async fn test_invalid_lifecycle_transition_rejected() {
    let (base, client, _db, _state, _dir) = spawn_server().await;

    // Register as pending, auto-activate, then decommission.
    client
        .post(format!("{base}/api/v1/machines/bad-trans/register"))
        .json(&serde_json::json!({ "lifecycle": "pending" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // Auto-activate via report.
    let report = Report {
        machine_id: "bad-trans".to_string(),
        current_generation: "/nix/store/gen1".to_string(),
        success: true,
        message: "ok".to_string(),
        timestamp: chrono::Utc::now(),
        tags: vec![],
        health: None,
    };
    client
        .post(format!("{base}/api/v1/machines/bad-trans/report"))
        .json(&report)
        .send()
        .await
        .unwrap();

    // Decommission.
    client
        .patch(format!("{base}/api/v1/machines/bad-trans/lifecycle"))
        .json(&serde_json::json!({ "lifecycle": "decommissioned" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // Attempt to re-activate a decommissioned machine -- must fail.
    let resp = client
        .patch(format!("{base}/api/v1/machines/bad-trans/lifecycle"))
        .json(&serde_json::json!({ "lifecycle": "active" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        409,
        "decommissioned -> active must be rejected"
    );
}

// ---- Audit Trail ------------------------------------------------------------

/// Registering a machine must produce an audit event.
#[tokio::test]
async fn test_audit_trail_on_register() {
    let (base, client, _db, _state, _dir) = spawn_server().await;

    client
        .post(format!("{base}/api/v1/machines/audit-host/register"))
        .json(&serde_json::json!({ "lifecycle": "pending" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let events: Vec<serde_json::Value> = client
        .get(format!("{base}/api/v1/audit?action=register"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["action"], "register");
    assert_eq!(events[0]["target"], "audit-host");
}

// ---- Audit CSV Export -------------------------------------------------------

#[tokio::test]
async fn test_audit_csv_export() {
    let (base, client, _db, _state, _dir) = spawn_server().await;

    // Create an audit event via registering a machine.
    client
        .post(format!("{base}/api/v1/machines/csv-host/register"))
        .json(&serde_json::json!({ "lifecycle": "pending" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let resp = client
        .get(format!("{base}/api/v1/audit/export"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body = resp.text().await.unwrap();
    assert!(body.starts_with("timestamp,actor,action,target,detail"));
    assert!(body.contains("register"));
    assert!(body.contains("csv-host"));
}

/// Decommissioned machines still appear in inventory but with decommissioned lifecycle.
#[tokio::test]
async fn test_decommissioned_machine_in_inventory() {
    let (base, client, _db, _state, _dir) = spawn_server().await;

    // Register and decommission.
    client
        .post(format!("{base}/api/v1/machines/decom-host/register"))
        .json(&serde_json::json!({ "lifecycle": "pending" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    client
        .patch(format!("{base}/api/v1/machines/decom-host/lifecycle"))
        .json(&serde_json::json!({ "lifecycle": "decommissioned" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    // Verify it appears with decommissioned lifecycle.
    let machines: Vec<MachineStatus> = client
        .get(format!("{base}/api/v1/machines"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let machine = machines
        .iter()
        .find(|m| m.machine_id == "decom-host")
        .unwrap();
    assert_eq!(
        machine.lifecycle,
        nixfleet_types::MachineLifecycle::Decommissioned,
    );
}
