//! `/v1/whoami` integration test (Phase 3 PR-2).
//!
//! Mints a synthetic fleet CA + server cert + client cert in-test
//! with rcgen, spins up `serve` with the CA wired as `--client-ca`
//! (mTLS-required mode), hits `/v1/whoami` with the client cert and
//! asserts the verified CN matches what we put in the cert.
//!
//! Also covers the negative case: same server, but a request
//! without a client cert is rejected at the TLS handshake. This is
//! the proof-of-life test for the mTLS pipeline before PR-3
//! wires the agent body.

use std::path::PathBuf;
use std::sync::Once;
use std::time::Duration;

use nixfleet_control_plane::server;
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use reqwest::{Certificate as ReqwestCert, Identity};
use serde::Deserialize;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::time::sleep;

fn install_crypto_provider_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

#[derive(Debug, Deserialize)]
struct WhoamiBody {
    cn: String,
    #[serde(rename = "issuedAt")]
    #[allow(dead_code)]
    issued_at: String,
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

/// Minimal stub inputs the reconcile loop expects to find. See the
/// `/healthz` test for the rationale: `/v1/whoami` doesn't depend on
/// the reconcile loop, but the serve loop spawns it regardless.
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

/// Build a self-signed CA, a server cert signed by it (SAN=localhost),
/// and a client cert signed by it (CN=`client_cn`). Returns the PEMs
/// written to `dir` (ca, server-cert, server-key, client-cert,
/// client-key) and the client CN string.
fn mint_ca_and_certs(
    dir: &TempDir,
    client_cn: &str,
) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {
    // CA
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
    let ca_pem = ca_cert.pem();

    // Server cert (SAN=localhost so rustls accepts the connection)
    let mut server_params =
        CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    server_params
        .distinguished_name
        .push(DnType::CommonName, "test-cp-server");
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_key = KeyPair::generate().unwrap();
    let server_cert = server_params.signed_by(&server_key, &ca_cert, &ca_key).unwrap();
    let server_pem = server_cert.pem();

    // Client cert (CN = client_cn, what /v1/whoami should return)
    let mut client_params = CertificateParams::default();
    client_params
        .distinguished_name
        .push(DnType::CommonName, client_cn);
    client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let client_key = KeyPair::generate().unwrap();
    let client_cert = client_params.signed_by(&client_key, &ca_cert, &ca_key).unwrap();
    let client_pem = client_cert.pem();

    let ca_path = write_pem(dir, "ca.pem", &ca_pem);
    let server_cert_path = write_pem(dir, "server.pem", &server_pem);
    let server_key_path = write_pem(dir, "server.key", &server_key.serialize_pem());
    let client_cert_path = write_pem(dir, "client.pem", &client_pem);
    let client_key_path = write_pem(dir, "client.key", &client_key.serialize_pem());

    (
        ca_path,
        server_cert_path,
        server_key_path,
        client_cert_path,
        client_key_path,
    )
}

async fn spawn_server(args: server::ServeArgs) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    let handle = tokio::spawn(server::serve(args));
    sleep(Duration::from_millis(200)).await;
    assert!(
        !handle.is_finished(),
        "server task exited before tests could run (TLS config error?)"
    );
    handle
}

#[tokio::test]
async fn whoami_returns_verified_cn_when_client_cert_present() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "krach");
    let (artifact, signature, trust, observed) = write_phase2_input_stubs(&dir);

    let port = pick_free_port().await;
    let listen = format!("127.0.0.1:{port}").parse().unwrap();

    let server_handle = spawn_server(server::ServeArgs {
        listen,
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

    // Build reqwest client with our client cert + key as Identity.
    let mut client_pem_bytes = std::fs::read(&client_cert).unwrap();
    client_pem_bytes.extend_from_slice(&std::fs::read(&client_key).unwrap());
    let identity = Identity::from_pem(&client_pem_bytes).unwrap();
    let ca_pem = std::fs::read(&ca).unwrap();
    let ca_cert = ReqwestCert::from_pem(&ca_pem).unwrap();
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(ca_cert)
        .identity(identity)
        .build()
        .unwrap();

    let url = format!("https://localhost:{port}/v1/whoami");
    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status(), 200);

    let body: WhoamiBody = resp.json().await.unwrap();
    assert_eq!(body.cn, "krach");

    server_handle.abort();
}

#[tokio::test]
async fn whoami_rejects_request_without_client_cert() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, _client_cert, _client_key) =
        mint_ca_and_certs(&dir, "krach");
    let (artifact, signature, trust, observed) = write_phase2_input_stubs(&dir);

    let port = pick_free_port().await;
    let listen = format!("127.0.0.1:{port}").parse().unwrap();

    let server_handle = spawn_server(server::ServeArgs {
        listen,
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

    // Same client config but NO identity — no client cert presented.
    // Server's WebPkiClientVerifier rejects the handshake; reqwest
    // surfaces that as a connect error.
    let ca_pem = std::fs::read(&ca).unwrap();
    let ca_cert = ReqwestCert::from_pem(&ca_pem).unwrap();
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(ca_cert)
        .build()
        .unwrap();

    let url = format!("https://localhost:{port}/v1/whoami");
    let result = client.get(&url).send().await;
    assert!(
        result.is_err(),
        "expected TLS handshake failure when client presents no cert, got: {result:?}"
    );

    server_handle.abort();
}
