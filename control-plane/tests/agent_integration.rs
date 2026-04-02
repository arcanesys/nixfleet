/// Integration tests for the NixFleet agent <-> control plane communication cycle.
///
/// Each test spins up a real Axum server on a random port (via `TcpListener::bind("127.0.0.1:0")`),
/// then drives it with `reqwest` -- the same HTTP semantics any real agent or operator would use.
///
/// Scenarios covered:
///   1. Happy-path deploy: set generation -> agent polls -> agent reports success.
///   2. Failed deploy: agent reports failure; inventory reflects "error" state.
///   3. Multi-machine isolation: three machines do not interfere with each other.
///   4. Generation upsert: setting twice keeps only the latest, no duplicates.
///   5. cache_url propagation: optional field round-trips through the API.
use nixfleet_control_plane::{build_app, db, state};
use nixfleet_types::{DesiredGeneration, MachineStatus, Report};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::RwLock;

const RAW_TEST_KEY: &str = "test-key";

// ---- Helpers ----------------------------------------------------------------

/// Spawns a real control-plane server on a random port.
///
/// Returns (base_url, authenticated_client, TempDir). The server runs on a background
/// Tokio task and is torn down when the test process exits. The `TempDir` must be kept
/// alive for the duration of the test to prevent the SQLite database from being deleted.
async fn spawn_server() -> (String, reqwest::Client, tempfile::TempDir) {
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
    let app = build_app(fleet_state, database);

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

    (format!("http://{addr}"), client, dir)
}

// ---- Auth -------------------------------------------------------------------

#[tokio::test]
async fn test_unauthenticated_request_rejected() {
    let (base, _client, _dir) = spawn_server().await;
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
    let (base, _client, _dir) = spawn_server().await;
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
    let (base, _client, _dir) = spawn_server().await;
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
    let (base, client, _dir) = spawn_server().await;
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

    // 2. Operator sets a desired generation.
    let set_resp = client
        .post(format!(
            "{base}/api/v1/machines/{machine_id}/set-generation"
        ))
        .json(&serde_json::json!({ "hash": gen_hash }))
        .send()
        .await
        .unwrap();
    assert_eq!(set_resp.status(), 200, "set-generation must succeed");

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
    let (base, client, _dir) = spawn_server().await;
    let machine_id = "dev-01";
    let bad_hash = "/nix/store/bad000-nixos-system-dev-01-25.05";
    let rollback_gen = "/nix/store/good111-nixos-system-dev-01-25.05";

    // Operator sets generation.
    client
        .post(format!(
            "{base}/api/v1/machines/{machine_id}/set-generation"
        ))
        .json(&serde_json::json!({ "hash": bad_hash }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

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
    let (base, client, _dir) = spawn_server().await;

    let machines = [
        ("web-01", "/nix/store/aaa-nixos-system-web-01"),
        ("dev-01", "/nix/store/bbb-nixos-system-dev-01"),
        ("mac-01", "/nix/store/ccc-nixos-system-mac-01"),
    ];

    // Set generations for all machines.
    for (id, hash) in &machines {
        client
            .post(format!("{base}/api/v1/machines/{id}/set-generation"))
            .json(&serde_json::json!({ "hash": hash }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
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

// ---- Generation upsert ------------------------------------------------------

/// Setting a generation twice must upsert (not duplicate) and return the latest.
#[tokio::test]
async fn test_set_generation_upsert() {
    let (base, client, _dir) = spawn_server().await;
    let machine_id = "srv-01";
    let gen_v1 = "/nix/store/v1-nixos-system";
    let gen_v2 = "/nix/store/v2-nixos-system";

    for gen in [gen_v1, gen_v2] {
        client
            .post(format!(
                "{base}/api/v1/machines/{machine_id}/set-generation"
            ))
            .json(&serde_json::json!({ "hash": gen }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }

    // Poll must return the second (latest) generation.
    let desired: DesiredGeneration = client
        .get(format!(
            "{base}/api/v1/machines/{machine_id}/desired-generation"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(desired.hash, gen_v2, "second set must win (upsert)");

    // Inventory must show exactly one entry for this machine.
    let list: Vec<MachineStatus> = client
        .get(format!("{base}/api/v1/machines"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let count = list.iter().filter(|m| m.machine_id == machine_id).count();
    assert_eq!(
        count, 1,
        "upsert must not create duplicate inventory entries"
    );
}

// ---- Optional cache_url propagation -----------------------------------------

/// cache_url set alongside a generation must round-trip through the API.
#[tokio::test]
async fn test_set_generation_with_cache_url() {
    let (base, client, _dir) = spawn_server().await;
    let machine_id = "cache-test";
    let hash = "/nix/store/cached-nixos-system";
    let cache_url = "https://cache.example.com";

    client
        .post(format!(
            "{base}/api/v1/machines/{machine_id}/set-generation"
        ))
        .json(&serde_json::json!({ "hash": hash, "cache_url": cache_url }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let desired: DesiredGeneration = client
        .get(format!(
            "{base}/api/v1/machines/{machine_id}/desired-generation"
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(desired.hash, hash);
    assert_eq!(
        desired.cache_url.as_deref(),
        Some(cache_url),
        "cache_url must be returned with the desired generation"
    );
}

// ---- Machine Registry -------------------------------------------------------

/// Pre-registering a machine sets it to Pending lifecycle.
#[tokio::test]
async fn test_register_machine() {
    let (base, client, _dir) = spawn_server().await;

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
    let (base, client, _dir) = spawn_server().await;

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
    let (base, client, _dir) = spawn_server().await;

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
    let (base, client, _dir) = spawn_server().await;

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

/// Setting a generation must produce an audit event.
#[tokio::test]
async fn test_audit_trail_on_set_generation() {
    let (base, client, _dir) = spawn_server().await;

    client
        .post(format!("{base}/api/v1/machines/web-01/set-generation"))
        .json(&serde_json::json!({"hash": "/nix/store/abc123"}))
        .send()
        .await
        .unwrap();

    let events: Vec<serde_json::Value> = client
        .get(format!("{base}/api/v1/audit?action=set_generation"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["action"], "set_generation");
    assert_eq!(events[0]["target"], "web-01");
}

// ---- Audit CSV Export -------------------------------------------------------

#[tokio::test]
async fn test_audit_csv_export() {
    let (base, client, _dir) = spawn_server().await;

    // Create an audit event via an action
    client
        .post(format!("{base}/api/v1/machines/web-01/set-generation"))
        .json(&serde_json::json!({"hash": "/nix/store/abc123"}))
        .send()
        .await
        .unwrap();

    let resp = client
        .get(format!("{base}/api/v1/audit/export"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body = resp.text().await.unwrap();
    assert!(body.starts_with("timestamp,actor,action,target,detail"));
    assert!(body.contains("set_generation"));
    assert!(body.contains("web-01"));
}

/// Decommissioned machines still appear in inventory but with decommissioned lifecycle.
#[tokio::test]
async fn test_decommissioned_machine_in_inventory() {
    let (base, client, _dir) = spawn_server().await;

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
