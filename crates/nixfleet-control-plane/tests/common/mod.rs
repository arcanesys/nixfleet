//! Shared TLS / cert / port helpers for CP integration tests.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Once;
use std::time::{Duration, Instant};

use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use reqwest::{Certificate as ReqwestCert, Identity};
use tempfile::TempDir;
use tokio::net::{TcpListener, TcpStream};

pub fn install_crypto_provider_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .try_init();
    });
}

// FOOTGUN: returns OS-allocated port; caller races the rebind in the spawn step.
pub async fn pick_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Poll TCP connect until success or deadline; panics if server task exits before binding.
pub async fn wait_for_listener_ready(
    port: u16,
    handle: &tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if handle.is_finished() {
            panic!(
                "server task exited before listener bound (likely TLS config error — check stderr)"
            );
        }
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("server listener on 127.0.0.1:{port} did not bind within 15s");
}

pub fn write_pem(dir: &TempDir, name: &str, contents: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

pub fn write_bytes(dir: &TempDir, name: &str, contents: &[u8]) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

/// Returns `(ca, server_cert, server_key, client_cert, client_key)` PEM paths.
pub fn mint_ca_and_certs(
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

    let mut server_params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    server_params
        .distinguished_name
        .push(DnType::CommonName, "test-cp-server");
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_key = KeyPair::generate().unwrap();
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .unwrap();

    let mut client_params = CertificateParams::default();
    client_params
        .distinguished_name
        .push(DnType::CommonName, client_cn);
    client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let client_key = KeyPair::generate().unwrap();
    let client_cert = client_params
        .signed_by(&client_key, &ca_cert, &ca_key)
        .unwrap();

    (
        write_pem(dir, "ca.pem", &ca_cert.pem()),
        write_pem(dir, "server.pem", &server_cert.pem()),
        write_pem(dir, "server.key", &server_key.serialize_pem()),
        write_pem(dir, "client.pem", &client_cert.pem()),
        write_pem(dir, "client.key", &client_key.serialize_pem()),
    )
}

pub fn build_mtls_client(
    ca: &PathBuf,
    client_cert: &PathBuf,
    client_key: &PathBuf,
) -> reqwest::Client {
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

/// (artifact, signature, trust, observed) PEM paths — empty/no-op stubs that
/// satisfy the `serve` startup checks without driving any real polling.
pub fn write_phase2_input_stubs(dir: &TempDir) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
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

/// Returns `(raw_json_to_write, canonical_bytes_to_sign)` for a single-host
/// `test-host`/`stable` fleet — the shape every CP integration test that
/// needs a live fleet uses.
pub fn build_fleet_resolved_json(declared_closure: &str, ci_commit: &str) -> (String, Vec<u8>) {
    let json = serde_json::json!({
        "schemaVersion": 1,
        "hosts": {
            "test-host": {
                "system": "x86_64-linux",
                "tags": [],
                "channel": "stable",
                "closureHash": declared_closure,
                "pubkey": null,
            }
        },
        "channels": {
            "stable": {
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
            "signedAt": "2026-04-26T00:00:00Z",
            "ciCommit": ci_commit,
            "signatureAlgorithm": "ed25519",
        },
    });
    let raw = serde_json::to_string(&json).unwrap();
    let canonical = nixfleet_canonicalize::canonicalize(&raw).unwrap();
    (raw, canonical.into_bytes())
}

/// Spawn `server::serve` and wait for its TCP listener to bind.
pub async fn spawn_server(
    args: nixfleet_control_plane::server::ServeArgs,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    let port = args.listen.port();
    let handle = tokio::spawn(nixfleet_control_plane::server::serve(args));
    wait_for_listener_ready(port, &handle).await;
    handle
}
