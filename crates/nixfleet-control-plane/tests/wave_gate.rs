//! Wave-staging compliance gate end-to-end: signed fleet + signed event over mTLS,
//! enforce-mode blocks (target=null), permissive-mode advisory (target populated).

mod common;

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use chrono::Utc;
use common::{
    build_mtls_client, install_crypto_provider_once, mint_ca_and_certs, pick_free_port,
    wait_for_listener_ready, write_bytes, write_pem,
};
use ed25519_dalek::{Signer, SigningKey};
use nixfleet_control_plane::server;
use nixfleet_proto::agent_wire::{
    CheckinRequest, CheckinResponse, FetchOutcome, FetchResult, GenerationRef, ReportEvent,
    ReportRequest, ReportResponse,
};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::Serialize;
use tempfile::TempDir;

const HOSTNAME: &str = "test-host";
const DECLARED_CLOSURE: &str = "decl0001-nixos-system-test-host-26.05";
const CURRENT_CLOSURE: &str = "curr0001-nixos-system-test-host-26.05";
const CI_COMMIT: &str = "abc12345deadbeefcafebabe";

fn fresh_host_keypair() -> (SigningKey, String) {
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    let sk = SigningKey::from_bytes(&seed);
    let pubkey_bytes = sk.verifying_key().to_bytes();
    let ssh_pk = ssh_key::PublicKey::new(
        ssh_key::public::KeyData::Ed25519(ssh_key::public::Ed25519PublicKey(pubkey_bytes)),
        "test-host",
    );
    let openssh = ssh_pk.to_openssh().expect("to_openssh");
    (sk, openssh)
}

fn write_signed_fleet(
    dir: &TempDir,
    compliance_mode: &str,
    host_ssh_pubkey: Option<&str>,
) -> (PathBuf, PathBuf, PathBuf) {
    let signing_key = SigningKey::generate(&mut OsRng);
    let public_b64 =
        base64::engine::general_purpose::STANDARD.encode(signing_key.verifying_key());

    let signed_at = "2026-04-26T00:00:00Z";
    let json = serde_json::json!({
        "schemaVersion": 1,
        "hosts": {
            HOSTNAME: {
                "system": "x86_64-linux",
                "tags": [],
                "channel": "stable",
                "closureHash": DECLARED_CLOSURE,
                "pubkey": host_ssh_pubkey,
            }
        },
        "channels": {
            "stable": {
                "rolloutPolicy": "default",
                "reconcileIntervalMinutes": 5,
                "freshnessWindow": 60,
                "signingIntervalMinutes": 30,
                "compliance": {
                    "frameworks": [],
                    "mode": compliance_mode,
                },
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
    let signature = signing_key.sign(canonical.as_bytes());

    let artifact = write_pem(dir, "fleet.resolved.json", &raw);
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
    wait_for_listener_ready(port, &handle).await;
    handle
}

fn checkin_request(current: &str) -> CheckinRequest {
    CheckinRequest {
        hostname: HOSTNAME.to_string(),
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ComplianceFailureSignedPayload<'a> {
    hostname: &'a str,
    rollout: Option<&'a str>,
    control_id: &'a str,
    status: &'a str,
    framework_articles: &'a [String],
    evidence_collected_at: chrono::DateTime<chrono::Utc>,
    evidence_snippet_sha256: String,
}

fn build_signed_compliance_failure(
    sk: &SigningKey,
    rollout: &str,
    control_id: &str,
) -> ReportRequest {
    let articles: Vec<String> = vec!["nis2:21(b)".to_string()];
    let snippet = serde_json::json!({"compliant": false, "rule": "AL-03"});
    let snippet_sha = nixfleet_canonicalize::sha256_jcs_hex(&snippet).unwrap();
    let evidence_collected_at = Utc::now();
    let payload = ComplianceFailureSignedPayload {
        hostname: HOSTNAME,
        rollout: Some(rollout),
        control_id,
        status: "non-compliant",
        framework_articles: &articles,
        evidence_collected_at,
        evidence_snippet_sha256: snippet_sha,
    };
    let canonical = serde_jcs::to_vec(&payload).unwrap();
    let signature = sk.sign(&canonical);
    let signature_b64 =
        base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());

    ReportRequest {
        hostname: HOSTNAME.to_string(),
        agent_version: "test".to_string(),
        occurred_at: Utc::now(),
        rollout: Some(rollout.to_string()),
        event: ReportEvent::ComplianceFailure {
            control_id: control_id.to_string(),
            status: "non-compliant".to_string(),
            framework_articles: articles,
            evidence_snippet: Some(snippet),
            evidence_collected_at,
            signature: Some(signature_b64),
        },
    }
}

fn build_signed_runtime_gate_error(sk: &SigningKey, rollout: &str) -> ReportRequest {
    use nixfleet_proto::evidence_signing::RuntimeGateErrorSignedPayload;

    let evidence_collected_at = Utc::now();
    let activation_completed_at = Utc::now();
    let reason = "evidence-stale";
    let collector_exit_code = Some(0);

    let payload = RuntimeGateErrorSignedPayload {
        hostname: HOSTNAME,
        rollout: Some(rollout),
        reason,
        collector_exit_code,
        evidence_collected_at: Some(evidence_collected_at),
        activation_completed_at,
    };
    let canonical = serde_jcs::to_vec(&payload).unwrap();
    let signature = sk.sign(&canonical);
    let signature_b64 =
        base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());

    ReportRequest {
        hostname: HOSTNAME.to_string(),
        agent_version: "test".to_string(),
        occurred_at: Utc::now(),
        rollout: Some(rollout.to_string()),
        event: ReportEvent::RuntimeGateError {
            reason: reason.to_string(),
            collector_exit_code,
            evidence_collected_at: Some(evidence_collected_at),
            activation_completed_at,
            signature: Some(signature_b64),
        },
    }
}

async fn post_checkin(
    client: &reqwest::Client,
    port: u16,
    req: &CheckinRequest,
) -> CheckinResponse {
    client
        .post(format!("https://localhost:{port}/v1/agent/checkin"))
        .json(req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn enforce_mode_blocks_dispatch_after_signed_compliance_failure() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (host_sk, host_pubkey) = fresh_host_keypair();
    let (artifact, signature, trust) =
        write_signed_fleet(&dir, "enforce", Some(&host_pubkey));
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, HOSTNAME);
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
        db_path,
        port,
    )
    .await;
    let client = build_mtls_client(&ca, &client_cert, &client_key);

    let checkin_diverged = checkin_request(CURRENT_CLOSURE);
    let resp1 = post_checkin(&client, port, &checkin_diverged).await;
    assert!(
        resp1.target.is_some(),
        "first checkin should dispatch (no outstanding events yet)"
    );
    let dispatched_rollout = resp1
        .target
        .as_ref()
        .map(|t| t.rollout_id.clone())
        .expect("dispatch carries rollout_id");

    let report = build_signed_compliance_failure(&host_sk, &dispatched_rollout, "auditLogging");
    let report_resp: ReportResponse = client
        .post(format!("https://localhost:{port}/v1/agent/report"))
        .json(&report)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        report_resp.event_id.starts_with("evt-"),
        "report accepted, got id: {}",
        report_resp.event_id
    );

    // GOTCHA: must echo rollout_id so wave gate's "host is on this rollout" lookup hits.
    let mut checkin_after_failure = checkin_request(CURRENT_CLOSURE);
    checkin_after_failure.last_evaluated_target =
        Some(nixfleet_proto::agent_wire::EvaluatedTarget {
            closure_hash: DECLARED_CLOSURE.to_string(),
            channel_ref: dispatched_rollout.clone(),
            evaluated_at: Utc::now(),
            rollout_id: dispatched_rollout.clone(),
            wave_index: None,
            activate: None,
            signed_at: Utc::now(),
            freshness_window_secs: 3600,
            compliance_mode: Some("enforce".to_string()),
        });
    let resp2 = post_checkin(&client, port, &checkin_after_failure).await;
    assert!(
        resp2.target.is_none(),
        "enforce + outstanding failure must block dispatch — got target {:?}",
        resp2.target
    );

    handle.abort();
}

/// LOADBEARING: gate must stay blocked across CP restart — boot-hydration of the
/// in-memory ring from `host_reports` is silent-fail-prone (serde drift would empty
/// the ring and unlock dispatch).
#[tokio::test]
async fn enforce_mode_still_blocks_dispatch_after_cp_restart() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (host_sk, host_pubkey) = fresh_host_keypair();
    let (artifact, signature, trust) =
        write_signed_fleet(&dir, "enforce", Some(&host_pubkey));
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, HOSTNAME);
    let db_path = dir.path().join("state.db");
    let port = pick_free_port().await;
    let client = build_mtls_client(&ca, &client_cert, &client_key);

    let handle1 = spawn_with_signed_fleet(
        &dir,
        artifact.clone(),
        signature.clone(),
        trust.clone(),
        server_cert.clone(),
        server_key.clone(),
        ca.clone(),
        db_path.clone(),
        port,
    )
    .await;

    let resp1 = post_checkin(&client, port, &checkin_request(CURRENT_CLOSURE)).await;
    let dispatched_rollout = resp1
        .target
        .as_ref()
        .map(|t| t.rollout_id.clone())
        .expect("first checkin dispatches");

    // LOADBEARING: must persist to SQLite host_reports so the second CP can rehydrate it.
    let report = build_signed_compliance_failure(&host_sk, &dispatched_rollout, "auditLogging");
    let report_resp: ReportResponse = client
        .post(format!("https://localhost:{port}/v1/agent/report"))
        .json(&report)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(report_resp.event_id.starts_with("evt-"));

    let mut checkin_after_failure = checkin_request(CURRENT_CLOSURE);
    checkin_after_failure.last_evaluated_target =
        Some(nixfleet_proto::agent_wire::EvaluatedTarget {
            closure_hash: DECLARED_CLOSURE.to_string(),
            channel_ref: dispatched_rollout.clone(),
            evaluated_at: Utc::now(),
            rollout_id: dispatched_rollout.clone(),
            wave_index: None,
            activate: None,
            signed_at: Utc::now(),
            freshness_window_secs: 3600,
            compliance_mode: Some("enforce".to_string()),
        });
    let resp_pre_restart = post_checkin(&client, port, &checkin_after_failure).await;
    assert!(
        resp_pre_restart.target.is_none(),
        "pre-restart sanity: gate must already be blocking — got {:?}",
        resp_pre_restart.target
    );

    // LOADBEARING: await the JoinHandle so SQLite file lock + listener slot are released.
    handle1.abort();
    let _ = handle1.await;

    let port2 = pick_free_port().await;
    let handle2 = spawn_with_signed_fleet(
        &dir,
        artifact,
        signature,
        trust,
        server_cert,
        server_key,
        ca.clone(),
        db_path,
        port2,
    )
    .await;

    let resp_post_restart = post_checkin(&client, port2, &checkin_after_failure).await;
    assert!(
        resp_post_restart.target.is_none(),
        "post-restart hydration broken: gate unlocked after CP restart — got target {:?}. \
         The host_reports SQLite row was not rehydrated into the gate's projection on \
         CP startup.",
        resp_post_restart.target
    );

    handle2.abort();
}

/// Parity with `enforce_mode_blocks_dispatch_after_signed_compliance_failure`,
/// but with a `RuntimeGateError` event (collector broke / evidence stale)
/// instead of a `ComplianceFailure` (a probe returned FAIL). Both kinds
/// land in `host_reports.event_kind IN ('compliance-failure',
/// 'runtime-gate-error')`; the SQL aggregator treats them identically.
/// This test pins the kind-agnostic gate behaviour at the integration
/// layer — without it, a regression that filtered out runtime-gate-error
/// events from the projection would silently unlock dispatch for hosts
/// whose evidence chain is broken (the "we couldn't measure" class).
#[tokio::test]
async fn enforce_mode_blocks_dispatch_after_signed_runtime_gate_error() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (host_sk, host_pubkey) = fresh_host_keypair();
    let (artifact, signature, trust) =
        write_signed_fleet(&dir, "enforce", Some(&host_pubkey));
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, HOSTNAME);
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
        db_path,
        port,
    )
    .await;
    let client = build_mtls_client(&ca, &client_cert, &client_key);

    let resp1 = post_checkin(&client, port, &checkin_request(CURRENT_CLOSURE)).await;
    assert!(
        resp1.target.is_some(),
        "first checkin should dispatch (no outstanding events yet)"
    );
    let dispatched_rollout = resp1
        .target
        .as_ref()
        .map(|t| t.rollout_id.clone())
        .expect("dispatch carries rollout_id");

    let report = build_signed_runtime_gate_error(&host_sk, &dispatched_rollout);
    let report_resp: ReportResponse = client
        .post(format!("https://localhost:{port}/v1/agent/report"))
        .json(&report)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        report_resp.event_id.starts_with("evt-"),
        "runtime-gate-error report accepted, got id: {}",
        report_resp.event_id
    );

    // GOTCHA: must echo rollout_id so wave gate's "host is on this rollout" lookup hits.
    let mut checkin_after_failure = checkin_request(CURRENT_CLOSURE);
    checkin_after_failure.last_evaluated_target =
        Some(nixfleet_proto::agent_wire::EvaluatedTarget {
            closure_hash: DECLARED_CLOSURE.to_string(),
            channel_ref: dispatched_rollout.clone(),
            evaluated_at: Utc::now(),
            rollout_id: dispatched_rollout.clone(),
            wave_index: None,
            activate: None,
            signed_at: Utc::now(),
            freshness_window_secs: 3600,
            compliance_mode: Some("enforce".to_string()),
        });
    let resp2 = post_checkin(&client, port, &checkin_after_failure).await;
    assert!(
        resp2.target.is_none(),
        "enforce + outstanding runtime-gate-error must block dispatch identically to \
         compliance-failure — got target {:?}",
        resp2.target
    );

    handle.abort();
}

#[tokio::test]
async fn permissive_mode_does_not_block_dispatch_despite_failure() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (host_sk, host_pubkey) = fresh_host_keypair();
    let (artifact, signature, trust) =
        write_signed_fleet(&dir, "permissive", Some(&host_pubkey));
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, HOSTNAME);
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
        db_path,
        port,
    )
    .await;
    let client = build_mtls_client(&ca, &client_cert, &client_key);

    let resp1 = post_checkin(&client, port, &checkin_request(CURRENT_CLOSURE)).await;
    let dispatched_rollout = resp1
        .target
        .as_ref()
        .map(|t| t.rollout_id.clone())
        .expect("first checkin dispatches");

    let report = build_signed_compliance_failure(&host_sk, &dispatched_rollout, "auditLogging");
    let _: ReportResponse = client
        .post(format!("https://localhost:{port}/v1/agent/report"))
        .json(&report)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // GOTCHA: observable contract under permissive is "no 500"; InFlight (target=None) is fine.
    let resp2 = post_checkin(&client, port, &checkin_request(CURRENT_CLOSURE)).await;
    assert_eq!(resp2.next_checkin_secs, 60);

    handle.abort();
}
