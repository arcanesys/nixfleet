//! `/v1/whoami` mTLS integration tests: verified-CN happy path + 401 without cert.

mod common;

use common::{
    install_crypto_provider_once, mint_ca_and_certs, pick_free_port, spawn_server,
    write_phase2_input_stubs,
};
use nixfleet_control_plane::server;
use reqwest::{Certificate as ReqwestCert, Identity};
use serde::Deserialize;
use tempfile::TempDir;

#[derive(Debug, Deserialize)]
struct WhoamiBody {
    cn: String,
    #[serde(rename = "issuedAt")]
    #[allow(dead_code)]
    issued_at: String,
}

#[tokio::test]
async fn whoami_returns_verified_cn_when_client_cert_present() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "test-host");
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
        confirm_deadline_secs: 120,
        ..Default::default()
    })
    .await;

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
    assert_eq!(body.cn, "test-host");

    server_handle.abort();
}

#[tokio::test]
async fn whoami_rejects_request_without_client_cert() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, _client_cert, _client_key) =
        mint_ca_and_certs(&dir, "test-host");
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
        confirm_deadline_secs: 120,
        ..Default::default()
    })
    .await;

    // LOADBEARING: TLS layer accepts unauthenticated connections so
    // `/v1/enroll` can bootstrap without a client cert; the 401 must come
    // from `require_cn` middleware at the route layer, not the handshake.
    let ca_pem = std::fs::read(&ca).unwrap();
    let ca_cert = ReqwestCert::from_pem(&ca_pem).unwrap();
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(ca_cert)
        .build()
        .unwrap();

    let url = format!("https://localhost:{port}/v1/whoami");
    let result = client.get(&url).send().await;
    let response = result.expect("TLS handshake should succeed without client cert");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "/v1/whoami without client cert must be rejected by require_cn middleware (401)",
    );

    server_handle.abort();
}
