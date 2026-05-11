//! Full-cycle integration: POST /v1/agent/checkin → /v1/agent/confirm
//! → GET /v1/rollouts/{id}/trace. Closes the deferred test from e549f63.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use common::{
    build_mtls_client, install_crypto_provider_once, mint_ca_and_certs, pick_free_port,
    wait_for_listener_ready, write_bytes, write_pem,
};
use ed25519_dalek::{Signer, SigningKey};
use nixfleet_control_plane::server;
use nixfleet_proto::agent_wire::{
    CheckinRequest, CheckinResponse, ConfirmRequest, FetchOutcome, FetchResult, GenerationRef,
};
use nixfleet_proto::RolloutTrace;
use rand::rngs::OsRng;
use tempfile::TempDir;

const HOST: &str = "trace-host";
const CHANNEL: &str = "stable";
const DECLARED_CLOSURE: &str = "decl-trace-closure-deadbeef";
const AGENT_CURRENT: &str = "running-system-old";
const CI_COMMIT: &str = "abcdef0011223344556677";

fn build_fleet_resolved_json() -> (String, Vec<u8>) {
    let signed_at = "2026-05-05T00:00:00Z";
    let json = serde_json::json!({
        "schemaVersion": 1,
        "hosts": {
            HOST: {
                "system": "x86_64-linux",
                "tags": [],
                "channel": CHANNEL,
                "closureHash": DECLARED_CLOSURE,
                "pubkey": null,
            }
        },
        "channels": {
            CHANNEL: {
                "rolloutPolicy": "default",
                "reconcileIntervalMinutes": 5,
                "freshnessWindow": 60,
                "signingIntervalMinutes": 30,
                "compliance": { "mode": "disabled", "frameworks": [] },
            }
        },
        "rolloutPolicies": {
            "default": {
                "strategy": "waves",
                "waves": [],
                "healthGate": {},
                "onHealthFailure": "halt",
            }
        },
        "waves": {},
        "edges": [],
        "disruptionBudgets": [],
        "meta": {
            "schemaVersion": 1,
            "signedAt": signed_at,
            "ciCommit": CI_COMMIT,
            "signatureAlgorithm": "ed25519",
        },
    });
    let raw = serde_json::to_string(&json).unwrap();
    let canonical = nixfleet_canonicalize::canonicalize(&raw).unwrap();
    (raw, canonical.into_bytes())
}

fn write_signed_fleet(dir: &TempDir) -> (PathBuf, PathBuf, PathBuf) {
    let signing_key = SigningKey::generate(&mut OsRng);
    let public_b64 = base64::engine::general_purpose::STANDARD.encode(signing_key.verifying_key());
    let (raw_json, canonical_bytes) = build_fleet_resolved_json();
    let signature = signing_key.sign(&canonical_bytes);
    let artifact = write_pem(dir, "fleet.resolved.json", &raw_json);
    let signature_path = write_bytes(dir, "fleet.resolved.json.sig", &signature.to_bytes());
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
async fn spawn_signed(
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
    wait_for_listener_ready(port, &handle).await;
    handle
}

fn checkin_request(current: &str) -> CheckinRequest {
    CheckinRequest {
        hostname: HOST.into(),
        agent_version: "test".into(),
        current_generation: GenerationRef {
            closure_hash: current.into(),
            channel_ref: None,
            boot_id: "00000000-0000-0000-0000-000000000000".into(),
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
async fn dispatch_then_confirm_then_trace_round_trips() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (artifact, signature, trust) = write_signed_fleet(&dir);
    let (ca, server_cert, server_key, client_cert, client_key) = mint_ca_and_certs(&dir, HOST);
    let db_path = dir.path().join("cp.db");
    let port = pick_free_port().await;

    let handle = spawn_signed(
        &dir,
        artifact,
        signature,
        trust,
        server_cert,
        server_key,
        ca.clone(),
        db_path,
        port,
    )
    .await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);

    // ---- 1. Checkin: agent's current ≠ declared → CP dispatches a target. ----
    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/checkin"))
        .json(&checkin_request(AGENT_CURRENT))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "/v1/agent/checkin");
    let checkin_body: CheckinResponse = resp.json().await.unwrap();
    let target = checkin_body
        .target
        .expect("non-converged checkin must dispatch a target");
    let rollout_id = target.rollout_id.clone();
    assert_eq!(target.closure_hash, DECLARED_CLOSURE);
    assert_eq!(rollout_id.len(), 64, "rolloutId is sha256-hex");

    // ---- 2. Confirm: flips host_dispatch_state → confirmed; transitions to Healthy. ----
    let confirm = ConfirmRequest {
        hostname: HOST.into(),
        rollout: rollout_id.clone(),
        wave: target.wave_index.unwrap_or(0),
        generation: GenerationRef {
            closure_hash: target.closure_hash.clone(),
            channel_ref: Some(target.channel_ref.clone()),
            boot_id: "00000000-0000-0000-0000-000000000001".into(),
        },
    };
    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/confirm"))
        .json(&confirm)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        204,
        "/v1/agent/confirm: expected 204, got {}",
        resp.status(),
    );

    // ---- 3. Trace: present rollout returns wave-major event list. ----
    let resp = client
        .get(format!(
            "https://localhost:{port}/v1/rollouts/{rollout_id}/trace"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "/trace for present rollout");
    let trace: RolloutTrace = resp
        .json()
        .await
        .expect("trace body must round-trip RolloutTrace");
    assert_eq!(trace.rollout_id, rollout_id, "echoed rolloutId");
    assert_eq!(trace.events.len(), 1, "exactly one dispatch_history row");
    let event = &trace.events[0];
    assert_eq!(event.host, HOST);
    assert_eq!(event.channel, CHANNEL);
    assert_eq!(event.target_closure_hash, DECLARED_CLOSURE);
    assert_eq!(event.target_channel_ref, target.channel_ref);
    // GOTCHA: confirm flips host_dispatch_state but does NOT terminalise
    // dispatch_history - that's the reconciler/supersession's job. So the
    // event reads "still open" until a converge sweep stamps it.
    assert!(
        event.terminal_state.is_none(),
        "confirm alone does not stamp dispatch_history.terminal_state; \
         got {:?}",
        event.terminal_state,
    );
    assert!(event.terminal_at.is_none());
    // dispatched_at is the SQL `datetime('now')` default - RFC3339-shaped
    // is contract; just sanity-check non-empty rather than parse.
    assert!(!event.dispatched_at.is_empty(), "dispatched_at populated");

    // ---- 4. Trace: unknown rollout_id → 404 (per route handler contract). ----
    let unknown = "0".repeat(64);
    let resp = client
        .get(format!(
            "https://localhost:{port}/v1/rollouts/{unknown}/trace"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        404,
        "/trace on rollout with no dispatch_history rows must 404",
    );

    handle.abort();
}
