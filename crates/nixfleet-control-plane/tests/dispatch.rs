//! Dispatch-loop integration smoke test against a real signed fleet + sqlite store.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use common::{
    build_fleet_resolved_json, build_mtls_client, install_crypto_provider_once, mint_ca_and_certs,
    pick_free_port, wait_for_listener_ready, write_bytes, write_pem,
};
use ed25519_dalek::{Signer, SigningKey};
use nixfleet_control_plane::server;
use nixfleet_proto::agent_wire::{
    CheckinRequest, CheckinResponse, FetchOutcome, FetchResult, GenerationRef,
};
use rand::rngs::OsRng;
use tempfile::TempDir;

fn write_signed_fleet(
    dir: &TempDir,
    declared_closure: &str,
    ci_commit: &str,
) -> (PathBuf, PathBuf, PathBuf) {
    let signing_key = SigningKey::generate(&mut OsRng);
    let public_b64 = base64::engine::general_purpose::STANDARD.encode(signing_key.verifying_key());

    let (raw_json, canonical_bytes) = build_fleet_resolved_json(declared_closure, ci_commit);
    let signature = signing_key.sign(&canonical_bytes);

    let artifact = write_pem(dir, "fleet.resolved.json", &raw_json);
    let signature_path = write_bytes(dir, "fleet.resolved.json.sig", &signature.to_bytes());
    // GOTCHA: KeySlot is `{current, previous}` not a list; schemaVersion required by TrustConfig.
    let trust_json = serde_json::json!({
        "schemaVersion": 1,
        "ciReleaseKey": {
            "current": { "algorithm": "ed25519", "public": public_b64 },
            "previous": null,
            "rejectBefore": null,
        },
        "cacheKeys": [],
        "orgRootKey": null,
    });
    let trust = write_pem(dir, "trust.json", &trust_json.to_string());

    (artifact, signature_path, trust)
}

#[allow(clippy::too_many_arguments)]
async fn spawn_with_signed_fleet(
    dir: &TempDir,
    artifact: PathBuf,
    signature: PathBuf,
    trust: PathBuf,
    server_cert: PathBuf,
    server_key: PathBuf,
    ca: PathBuf,
    db_path: PathBuf,
    port: u16,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    let observed = write_pem(
        dir,
        "observed.json",
        r#"{"channelRefs":{},"lastRolledRefs":{},"hostState":{},"activeRollouts":[]}"#,
    );
    let listen: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let args = server::ServeArgs {
        listen,
        tls_cert: server_cert,
        tls_key: server_key,
        client_ca: Some(ca),
        artifact_path: artifact,
        signature_path: signature,
        trust_path: trust,
        observed_path: observed,
        freshness_window: Duration::from_secs(86400 * 365 * 5),
        confirm_deadline_secs: 120,
        db_path: Some(db_path),
        mark_ready_at_startup: true,
        ..Default::default()
    };
    let handle = tokio::spawn(server::serve(args));
    // LOADBEARING: prime-path verify-artifact + verified_fleet write completes before listener binds.
    wait_for_listener_ready(port, &handle).await;
    handle
}

const DECLARED_CLOSURE: &str = "decl0001-nixos-system-test-host-26.05";
const CI_COMMIT: &str = "abc12345deadbeefcafebabe";

fn checkin_request(current: &str) -> CheckinRequest {
    CheckinRequest {
        hostname: "test-host".to_string(),
        agent_version: "test".to_string(),
        current_generation: GenerationRef {
            closure_hash: current.to_string(),
            channel_ref: None,
            boot_id: "00000000-0000-0000-0000-000000000000".to_string(),
        },
        pending_generation: None,
        last_evaluated_target: None,
        last_fetch_outcome: Some(FetchOutcome {
            result: FetchResult::Ok,
            error: None,
        }),
        uptime_secs: None,
        last_confirmed_at: None,
        attestation_signature: None,
        health_probes: vec![],
        health_check_mode: None,
    }
}

#[tokio::test]
async fn dispatch_end_to_end_signed_fleet_then_idempotent() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (artifact, signature, trust) = write_signed_fleet(&dir, DECLARED_CLOSURE, CI_COMMIT);
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "test-host");
    let db_path = dir.path().join("state.db");
    let port = pick_free_port().await;

    let handle = spawn_with_signed_fleet(
        &dir,
        artifact,
        signature,
        trust,
        server_cert,
        server_key,
        ca.clone(),
        db_path.clone(),
        port,
    )
    .await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);

    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/checkin"))
        .json(&checkin_request("running-system-old"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: CheckinResponse = resp.json().await.unwrap();
    let target = body.target.expect("first checkin should dispatch a target");
    assert_eq!(target.closure_hash, DECLARED_CLOSURE);
    // GOTCHA: rolloutId/channel_ref are sha256 hex over the projected RolloutManifest; assert shape only.
    assert_eq!(target.channel_ref.len(), 64);
    assert!(
        target
            .channel_ref
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "channel_ref must be hex lowercase: {}",
        target.channel_ref,
    );
    assert_eq!(target.rollout_id, target.channel_ref);

    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/checkin"))
        .json(&checkin_request("running-system-old"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: CheckinResponse = resp.json().await.unwrap();
    assert!(
        body.target.is_none(),
        "second checkin while pending: target must be null",
    );

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM host_dispatch_state WHERE hostname = ?1",
            rusqlite::params!["test-host"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1, "expected exactly one host_dispatch_state row");

    handle.abort();
}

/// LOADBEARING regression: when target == current (Decision::Converged),
/// the CP materialises rollout-layer rows so the reconciler can see the
/// host as Soaked-equivalent. That insert is APPEND-ONLY on
/// `dispatch_history`, so naïve repeat-fires would leak a row on every
/// checkin (~5 hosts × 2 rollouts × 30s checkin = unbounded growth).
///
/// This test exercises the production guard (host_state probe → skip
/// if already Converged) by sending TWO checkins where current matches
/// the declared closure. After the first, the CP should have one
/// dispatch_history row + one host_rollout_state row in `Converged`.
/// After the second, the count must be unchanged - the guard short-
/// circuited the materialisation.
#[tokio::test]
async fn converged_at_dispatch_does_not_leak_dispatch_history_rows() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (artifact, signature, trust) = write_signed_fleet(&dir, DECLARED_CLOSURE, CI_COMMIT);
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "test-host");
    let db_path = dir.path().join("state.db");
    let port = pick_free_port().await;

    let handle = spawn_with_signed_fleet(
        &dir,
        artifact,
        signature,
        trust,
        server_cert,
        server_key,
        ca.clone(),
        db_path.clone(),
        port,
    )
    .await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);

    // Checkin #1: agent reports current == declared → Decision::Converged.
    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/checkin"))
        .json(&checkin_request(DECLARED_CLOSURE))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: CheckinResponse = resp.json().await.unwrap();
    assert!(
        body.target.is_none(),
        "Decision::Converged returns no target - agent has nothing to confirm",
    );

    let count_open_rows = || -> i64 {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM dispatch_history
             WHERE hostname = ?1 AND terminal_state IS NULL",
            rusqlite::params!["test-host"],
            |r| r.get(0),
        )
        .unwrap()
    };
    let after_first = count_open_rows();
    assert_eq!(
        after_first, 1,
        "first converged-at-dispatch materialises exactly one dispatch_history row",
    );

    // Verify host_rollout_state is in Converged so the guard will trip.
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let state: String = conn
            .query_row(
                "SELECT host_state FROM host_rollout_state WHERE hostname = ?1",
                rusqlite::params!["test-host"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "Converged");
    }

    // Checkin #2 (and onwards): same current, same target. Without the
    // guard this would insert row after row.
    for _ in 0..3 {
        let resp = client
            .post(format!("https://localhost:{port}/v1/agent/checkin"))
            .json(&checkin_request(DECLARED_CLOSURE))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    let after_repeats = count_open_rows();
    assert_eq!(
        after_repeats, after_first,
        "guard must skip materialisation when host_rollout_state is already Converged",
    );

    handle.abort();
}

/// Concurrent companion to `converged_at_dispatch_does_not_leak_dispatch_history_rows`.
///
/// The sequential test pins the host_state-probe guard under repeat
/// invocation. This one pins the same guard under N parallel checkins
/// arriving simultaneously - no caller has seen the probe yet, so each
/// concurrent task believes it must materialise. The host_rollout_state
/// UNIQUE constraint and the surrounding txn must serialise the writes;
/// only one row may land.
///
/// If a future refactor accidentally makes the materialisation lockless
/// (e.g., reads host_state outside the insert txn), this test will fail
/// with N>1 dispatch_history rows. Cheap insurance against the race
/// class the guard is supposed to close.
#[tokio::test]
async fn converged_at_dispatch_is_idempotent_under_concurrent_checkins() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (artifact, signature, trust) = write_signed_fleet(&dir, DECLARED_CLOSURE, CI_COMMIT);
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "test-host");
    let db_path = dir.path().join("state.db");
    let port = pick_free_port().await;

    let handle = spawn_with_signed_fleet(
        &dir,
        artifact,
        signature,
        trust,
        server_cert,
        server_key,
        ca.clone(),
        db_path.clone(),
        port,
    )
    .await;

    // Single mTLS client, shared connection pool. Reqwest will fan
    // requests out across HTTP/2 streams or parallel TCP - either
    // is the race we want to exercise.
    let client = build_mtls_client(&ca, &client_cert, &client_key);
    let url = format!("https://localhost:{port}/v1/agent/checkin");

    // Fire N concurrent checkins and wait for ALL to settle. Use
    // futures::join_all-equivalent via tokio::spawn so each request
    // runs on the runtime's threadpool independently.
    const N: usize = 8;
    let mut tasks = Vec::with_capacity(N);
    for _ in 0..N {
        let client = client.clone();
        let url = url.clone();
        tasks.push(tokio::spawn(async move {
            client
                .post(&url)
                .json(&checkin_request(DECLARED_CLOSURE))
                .send()
                .await
                .map(|r| r.status())
        }));
    }
    for t in tasks {
        let status = t.await.unwrap().unwrap();
        assert_eq!(
            status, 200,
            "every concurrent checkin must return 200 - converged-at-dispatch is non-mutating from the agent's perspective",
        );
    }

    // Assertion 1: exactly one open dispatch_history row. The race
    // window between "probe says no row" and "insert row" must be
    // closed by the UNIQUE constraint + atomic txn.
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let history_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM dispatch_history
             WHERE hostname = ?1 AND terminal_state IS NULL",
            rusqlite::params!["test-host"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        history_rows, 1,
        "concurrent converged-at-dispatch must produce EXACTLY one open dispatch_history row \
         (got {history_rows}). If this fails, the guard is not race-safe - multiple concurrent \
         checkins materialised in parallel.",
    );

    // Assertion 2: host_rollout_state ends in Converged.
    let state: String = conn
        .query_row(
            "SELECT host_state FROM host_rollout_state WHERE hostname = ?1",
            rusqlite::params!["test-host"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        state, "Converged",
        "concurrent converged-at-dispatch must leave host_rollout_state at Converged",
    );

    // Assertion 3: exactly one host_rollout_state row (UNIQUE constraint).
    let rollout_state_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM host_rollout_state WHERE hostname = ?1",
            rusqlite::params!["test-host"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        rollout_state_rows, 1,
        "concurrent checkins must not produce duplicate host_rollout_state rows",
    );

    handle.abort();
}
