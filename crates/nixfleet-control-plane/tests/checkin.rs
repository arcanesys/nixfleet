//! Integration tests for `/v1/agent/checkin` + `/v1/agent/report`.

mod common;

use chrono::Utc;
use common::{
    build_mtls_client, install_crypto_provider_once, mint_ca_and_certs, pick_free_port,
    spawn_server, write_phase2_input_stubs,
};
use nixfleet_control_plane::server;
use nixfleet_proto::agent_wire::{
    CheckinRequest, CheckinResponse, FetchOutcome, FetchResult, GenerationRef,
    ReportEvent, ReportRequest, ReportResponse,
};
use tempfile::TempDir;

#[tokio::test]
async fn checkin_records_request_and_returns_null_target() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "test-host");
    let (artifact, signature, trust, observed) = write_phase2_input_stubs(&dir);

    let port = pick_free_port().await;
    let server_handle = spawn_server(server::ServeArgs {
        listen: format!("127.0.0.1:{port}").parse().unwrap(),
        tls_cert: server_cert,
        tls_key: server_key,
        client_ca: Some(ca.clone()),
        artifact_path: artifact,
        signature_path: signature,
        trust_path: trust,
        observed_path: observed,
        confirm_deadline_secs: 120,
        ..Default::default()
    })
    .await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);

    let req = CheckinRequest {
        hostname: "test-host".to_string(),
        agent_version: "0.2.0".to_string(),
        current_generation: GenerationRef {
            closure_hash: "abc123".to_string(),
            channel_ref: None,
            boot_id: "00000000-0000-0000-0000-000000000000".to_string(),
        },
        pending_generation: None,
        last_evaluated_target: None,
        last_fetch_outcome: Some(FetchOutcome {
            result: FetchResult::Ok,
            error: None,
        }),
        uptime_secs: Some(42),
        last_confirmed_at: None,
        attestation_signature: None,
        health_probes: vec![],
        health_check_mode: None,
    };

    let resp: CheckinResponse = client
        .post(format!("https://localhost:{port}/v1/agent/checkin"))
        .json(&req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert!(resp.target.is_none(), "should not dispatch in this scenario");
    assert_eq!(resp.next_checkin_secs, 60);

    server_handle.abort();
}

#[tokio::test]
async fn checkin_rejects_cn_hostname_mismatch() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "test-host");
    let (artifact, signature, trust, observed) = write_phase2_input_stubs(&dir);

    let port = pick_free_port().await;
    let server_handle = spawn_server(server::ServeArgs {
        listen: format!("127.0.0.1:{port}").parse().unwrap(),
        tls_cert: server_cert,
        tls_key: server_key,
        client_ca: Some(ca.clone()),
        artifact_path: artifact,
        signature_path: signature,
        trust_path: trust,
        observed_path: observed,
        confirm_deadline_secs: 120,
        ..Default::default()
    })
    .await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);

    let req = CheckinRequest {
        hostname: "ohm".to_string(),
        agent_version: "0.2.0".to_string(),
        current_generation: GenerationRef {
            closure_hash: "abc123".to_string(),
            channel_ref: None,
            boot_id: "00000000-0000-0000-0000-000000000000".to_string(),
        },
        pending_generation: None,
        last_evaluated_target: None,
        last_fetch_outcome: None,
        uptime_secs: None,
        last_confirmed_at: None,
        attestation_signature: None,
        health_probes: vec![],
        health_check_mode: None,
    };

    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/checkin"))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    server_handle.abort();
}

#[tokio::test]
async fn report_records_event_and_returns_event_id() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "test-host");
    let (artifact, signature, trust, observed) = write_phase2_input_stubs(&dir);

    let port = pick_free_port().await;
    let server_handle = spawn_server(server::ServeArgs {
        listen: format!("127.0.0.1:{port}").parse().unwrap(),
        tls_cert: server_cert,
        tls_key: server_key,
        client_ca: Some(ca.clone()),
        artifact_path: artifact,
        signature_path: signature,
        trust_path: trust,
        observed_path: observed,
        confirm_deadline_secs: 120,
        ..Default::default()
    })
    .await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);

    let req = ReportRequest {
        hostname: "test-host".to_string(),
        agent_version: "0.2.0".to_string(),
        occurred_at: Utc::now(),
        rollout: Some("stable@abc12345".to_string()),
        event: ReportEvent::RealiseFailed {
            closure_hash: "abc123".to_string(),
            reason: "substituter 503 — upstream unavailable".to_string(),
            signature: None,
        },
    };

    let resp: ReportResponse = client
        .post(format!("https://localhost:{port}/v1/agent/report"))
        .json(&req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert!(
        resp.event_id.starts_with("evt-"),
        "expected evt-* event_id, got: {}",
        resp.event_id
    );

    server_handle.abort();
}
