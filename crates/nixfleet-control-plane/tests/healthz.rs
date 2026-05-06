//! Integration test for `/healthz` over TLS.

mod common;

use common::{
    install_crypto_provider_once, pick_free_port, wait_for_listener_ready,
    write_phase2_input_stubs, write_pem,
};
use nixfleet_control_plane::server;
use rcgen::{generate_simple_self_signed, CertifiedKey};
use reqwest::Certificate;
use serde::Deserialize;
use tempfile::TempDir;

#[derive(Debug, Deserialize)]
struct HealthzBody {
    ok: bool,
    version: String,
    #[serde(rename = "lastTickAt")]
    last_tick_at: Option<String>,
}

#[tokio::test]
async fn healthz_returns_ok_over_tls() {
    install_crypto_provider_once();

    // FOOTGUN: rustls rejects CN-only certs; SAN=localhost is required.
    let CertifiedKey { cert, key_pair } =
        generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();

    let dir = TempDir::new().unwrap();
    let cert_path = write_pem(&dir, "server.pem", &cert.pem());
    let key_path = write_pem(&dir, "server.key", &key_pair.serialize_pem());
    let (artifact, signature, trust, observed) = write_phase2_input_stubs(&dir);

    let port = pick_free_port().await;
    let listen = format!("127.0.0.1:{port}").parse().unwrap();

    let server_args = server::ServeArgs {
        listen,
        tls_cert: cert_path,
        tls_key: key_path,
        artifact_path: artifact,
        signature_path: signature,
        trust_path: trust,
        observed_path: observed,
        confirm_deadline_secs: 120,
        ..Default::default()
    };
    let server_handle = tokio::spawn(server::serve(server_args));

    wait_for_listener_ready(port, &server_handle).await;

    let cert_pem = cert.pem();
    let ca = Certificate::from_pem(cert_pem.as_bytes()).unwrap();
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(ca)
        .build()
        .unwrap();

    let url = format!("https://localhost:{port}/healthz");
    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status(), 200);

    let body: HealthzBody = resp.json().await.unwrap();
    assert!(body.ok);
    assert!(!body.version.is_empty(), "version should be populated");
    let _ = body.last_tick_at;

    server_handle.abort();
}
