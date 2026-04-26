//! `/v1/agent/checkin` + `/v1/agent/report` integration test (PR-3).
//!
//! Spins up an in-process server with mTLS, sends a checkin from a
//! client cert with CN=krach, asserts the body shape comes through
//! and CN-vs-hostname mismatch is rejected.
//!
//! Shares the cert-minting helpers with `whoami.rs` — duplicated
//! rather than abstracted to keep each test file standalone (cargo
//! integration tests can't share a `mod common`).

use std::path::PathBuf;
use std::sync::Once;
use std::time::Duration;

use chrono::Utc;
use nixfleet_control_plane::server;
use nixfleet_proto::agent_wire::{
    CheckinRequest, CheckinResponse, FetchOutcome, FetchResult, GenerationRef,
    ReportKind, ReportRequest, ReportResponse,
};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use reqwest::{Certificate as ReqwestCert, Identity};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::time::sleep;

fn install_crypto_provider_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

async fn pick_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn write_pem(dir: &TempDir, name: &str, contents: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn write_phase2_input_stubs(dir: &TempDir) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let artifact = write_pem(dir, "fleet.resolved.json", "{}");
    let signature = write_pem(dir, "fleet.resolved.json.sig", "");
    let trust = write_pem(
        dir,
        "trust.json",
        r#"{"ciReleaseKey":{"current":[],"rejectBefore":null}}"#,
    );
    let observed = write_pem(
        dir,
        "observed.json",
        r#"{"channelRefs":{},"lastRolledRefs":{},"hostState":{},"activeRollouts":[]}"#,
    );
    (artifact, signature, trust, observed)
}

fn mint_ca_and_certs(
    dir: &TempDir,
    client_cn: &str,
) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "test-fleet-ca");
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let ca_key = KeyPair::generate().unwrap();
    let ca_cert: Certificate = ca_params.self_signed(&ca_key).unwrap();

    let mut server_params =
        CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    server_params
        .distinguished_name
        .push(DnType::CommonName, "test-cp-server");
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_key = KeyPair::generate().unwrap();
    let server_cert = server_params.signed_by(&server_key, &ca_cert, &ca_key).unwrap();

    let mut client_params = CertificateParams::default();
    client_params
        .distinguished_name
        .push(DnType::CommonName, client_cn);
    client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let client_key = KeyPair::generate().unwrap();
    let client_cert = client_params.signed_by(&client_key, &ca_cert, &ca_key).unwrap();

    (
        write_pem(dir, "ca.pem", &ca_cert.pem()),
        write_pem(dir, "server.pem", &server_cert.pem()),
        write_pem(dir, "server.key", &server_key.serialize_pem()),
        write_pem(dir, "client.pem", &client_cert.pem()),
        write_pem(dir, "client.key", &client_key.serialize_pem()),
    )
}

async fn spawn_server(args: server::ServeArgs) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    let handle = tokio::spawn(server::serve(args));
    sleep(Duration::from_millis(200)).await;
    assert!(!handle.is_finished(), "server task exited prematurely");
    handle
}

fn build_mtls_client(ca: &PathBuf, client_cert: &PathBuf, client_key: &PathBuf) -> reqwest::Client {
    let mut pem = std::fs::read(client_cert).unwrap();
    pem.extend_from_slice(&std::fs::read(client_key).unwrap());
    let identity = Identity::from_pem(&pem).unwrap();
    let ca_pem = std::fs::read(ca).unwrap();
    let ca_cert = ReqwestCert::from_pem(&ca_pem).unwrap();
    reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(ca_cert)
        .identity(identity)
        .build()
        .unwrap()
}

#[tokio::test]
async fn checkin_records_request_and_returns_null_target() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "krach");
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
        freshness_window: Duration::from_secs(86400),
        forgejo: None,
        fleet_ca_cert: None,
        fleet_ca_key: None,
        audit_log_path: None,
    })
    .await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);

    let req = CheckinRequest {
        hostname: "krach".to_string(),
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
    };

    let resp: CheckinResponse = client
        .post(&format!("https://localhost:{port}/v1/agent/checkin"))
        .json(&req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert!(resp.target.is_none(), "Phase 3 should never dispatch");
    assert_eq!(resp.next_checkin_secs, 60);

    server_handle.abort();
}

#[tokio::test]
async fn checkin_rejects_cn_hostname_mismatch() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "krach");
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
        freshness_window: Duration::from_secs(86400),
        forgejo: None,
        fleet_ca_cert: None,
        fleet_ca_key: None,
        audit_log_path: None,
    })
    .await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);

    // Cert CN is "krach"; body claims to be "ohm". CP rejects.
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
    };

    let resp = client
        .post(&format!("https://localhost:{port}/v1/agent/checkin"))
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
        mint_ca_and_certs(&dir, "krach");
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
        freshness_window: Duration::from_secs(86400),
        forgejo: None,
        fleet_ca_cert: None,
        fleet_ca_key: None,
        audit_log_path: None,
    })
    .await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);

    let req = ReportRequest {
        hostname: "krach".to_string(),
        agent_version: "0.2.0".to_string(),
        kind: ReportKind::FetchFailed,
        error: Some("attic 503 — upstream unavailable".to_string()),
        context: Some(serde_json::json!({"closureHash": "abc123"})),
        occurred_at: Utc::now(),
    };

    let resp: ReportResponse = client
        .post(&format!("https://localhost:{port}/v1/agent/report"))
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
