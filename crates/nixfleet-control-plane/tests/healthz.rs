//! `/healthz` integration test (Phase 3 PR-1).
//!
//! Mints a self-signed server cert with rcgen, spins up the long-
//! running serve loop in-process on an ephemeral port, hits `/healthz`
//! over TLS with reqwest (CA-pinned to the rcgen cert), asserts 200 +
//! the documented JSON shape.
//!
//! This is the substrate every subsequent Phase 3 PR extends: PR-2
//! adds an mTLS variant, PR-3 spins up an in-process agent against
//! it, etc. Keeping this test focused (just /healthz) keeps the diff
//! readable in PR review.

use std::path::PathBuf;
use std::sync::Once;
use std::time::Duration;

use nixfleet_control_plane::server;
use rcgen::{generate_simple_self_signed, CertifiedKey};
use reqwest::Certificate;
use serde::Deserialize;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::time::sleep;

/// Once-per-process rustls CryptoProvider install. Mirrors the
/// production `install_crypto_provider` in src/main.rs — without it,
/// `tls::build_server_config`'s ServerConfig::builder() panics under
/// the test harness.
fn install_crypto_provider_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

#[derive(Debug, Deserialize)]
struct HealthzBody {
    ok: bool,
    version: String,
    #[serde(rename = "lastTickAt")]
    last_tick_at: Option<String>,
}

/// Find a free TCP port by binding to :0 and reading the assigned
/// port. The listener is dropped immediately; there's a small race
/// window before the server takes the port, but it's fine for tests.
async fn pick_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

fn write_pem(dir: &TempDir, name: &str, contents: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

/// Minimal Phase 2 inputs the reconcile loop expects to find. `tick`
/// will fail to parse a non-existent artifact, but the failure is
/// logged-not-fatal — the listener stays up. `/healthz` doesn't
/// depend on tick succeeding.
fn write_phase2_input_stubs(dir: &TempDir) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    // Empty files — the server logs read errors and continues. We
    // only need the paths to exist so the unit's
    // `ConditionPathExists` would pass at deploy time; the server
    // itself doesn't require non-empty inputs to bind the listener.
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

#[tokio::test]
async fn healthz_returns_ok_over_tls() {
    install_crypto_provider_once();

    // Self-signed cert with SAN = localhost so the rustls server
    // accepts connections to 127.0.0.1 (rustls validates SAN, CN-only
    // certs no longer work on modern stacks).
    let CertifiedKey { cert, key_pair } =
        generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();

    let dir = TempDir::new().unwrap();
    let cert_path = write_pem(&dir, "server.pem", &cert.pem());
    let key_path = write_pem(&dir, "server.key", &key_pair.serialize_pem());
    let (artifact, signature, trust, observed) = write_phase2_input_stubs(&dir);

    let port = pick_free_port().await;
    let listen = format!("127.0.0.1:{port}").parse().unwrap();

    // Spawn the server. It runs forever; the test drops the JoinHandle
    // when it's done, killing the runtime task on tokio runtime
    // shutdown at end-of-test.
    let server_args = server::ServeArgs {
        listen,
        tls_cert: cert_path,
        tls_key: key_path,
        client_ca: None,
        artifact_path: artifact,
        signature_path: signature,
        trust_path: trust,
        observed_path: observed,
        freshness_window: Duration::from_secs(86400),
        forgejo: None,
        fleet_ca_cert: None,
        fleet_ca_key: None,
        audit_log_path: None,
    };
    let server_handle = tokio::spawn(server::serve(server_args));

    // Give the listener time to bind. Polling a TCP connect would be
    // tighter, but a small fixed sleep is fine for a test.
    sleep(Duration::from_millis(200)).await;
    assert!(
        !server_handle.is_finished(),
        "server task exited before /healthz could be hit (likely TLS config error — check stderr)"
    );

    // CA-pinned reqwest client. The server's self-signed cert IS the
    // trust anchor in this test.
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
    // last_tick_at can be None (reconcile loop hasn't fired yet — first
    // tick is offset by RECONCILE_INTERVAL = 30s) or Some (longer test
    // run). Either is correct.
    let _ = body.last_tick_at;

    server_handle.abort();
}
