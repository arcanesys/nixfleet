//! #95 — `/v1/*` returns 503 + `Retry-After` until the daemon primes its
//! verified-fleet snapshot. `/healthz` (outside `/v1/*`) stays unguarded.

mod common;

use common::{
    install_crypto_provider_once, pick_free_port, wait_for_listener_ready, write_pem,
    write_phase2_input_stubs,
};
use nixfleet_control_plane::server;
use rcgen::{generate_simple_self_signed, CertifiedKey};
use reqwest::Certificate;
use tempfile::TempDir;

/// Default `mark_ready_at_startup = false` + bogus stub artifact ⇒ daemon
/// never primes ⇒ `/v1/*` returns 503 with `Retry-After: 30` and the
/// canonical JSON error body. `/healthz` stays unguarded so monitoring
/// can scrape the daemon while it boots.
///
/// LOADBEARING for #95: without this gate the daemon would serve
/// dispatch off whatever stale state the listener picked up at startup.
#[tokio::test]
async fn not_ready_serves_503_on_v1_but_keeps_healthz_open() {
    install_crypto_provider_once();

    let CertifiedKey { cert, key_pair } =
        generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();

    let dir = TempDir::new().unwrap();
    let cert_path = write_pem(&dir, "server.pem", &cert.pem());
    let key_path = write_pem(&dir, "server.key", &key_pair.serialize_pem());
    // write_phase2_input_stubs writes invalid (`{}`) artifact bytes, so the
    // build-time prime will fail — exactly the not-ready boot path #95 covers.
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
        // Deliberately NOT set — exercise the production not-ready path.
        // mark_ready_at_startup: false (default),
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

    // /healthz must serve 200 even though /v1/* is gated.
    let healthz_resp = client
        .get(format!("https://localhost:{port}/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        healthz_resp.status(),
        200,
        "healthz must stay open while not ready",
    );

    // /v1/whoami (mTLS-protected normally; here the readiness layer wraps
    // the auth layer, so 503 takes precedence over 401 — agents see a
    // consistent "come back later" signal regardless of cert posture).
    let v1_resp = client
        .get(format!("https://localhost:{port}/v1/whoami"))
        .send()
        .await
        .unwrap();
    assert_eq!(v1_resp.status(), 503, "/v1/* must 503 until ready");
    assert_eq!(
        v1_resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .map(|v| v.to_str().unwrap()),
        Some("30"),
        "Retry-After hint must align agents' poll cadence with the daemon's",
    );

    let body: serde_json::Value = v1_resp.json().await.unwrap();
    assert_eq!(body["error"], "control plane not ready");
    assert_eq!(body["reason"], "awaiting first signed artifact");

    // /v1/enroll is anonymous in the production router but still under
    // /v1/*; the ready gate covers it too. Strict trust footprint —
    // bootstrap can't proceed against a half-initialised daemon.
    let enroll_resp = client
        .post(format!("https://localhost:{port}/v1/enroll"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        enroll_resp.status(),
        503,
        "/v1/enroll must 503 until ready (no half-initialised bootstrap)",
    );

    server_handle.abort();
}

/// `mark_ready_at_startup = true` ⇒ `/v1/*` reaches its normal handlers
/// (which then enforce mTLS, version headers, etc.). 401 here proves the
/// readiness layer didn't mask auth — it simply opened the gate.
#[tokio::test]
async fn ready_at_startup_lets_v1_reach_auth_layer() {
    install_crypto_provider_once();

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
        mark_ready_at_startup: true,
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

    let v1_resp = client
        .get(format!("https://localhost:{port}/v1/whoami"))
        .send()
        .await
        .unwrap();
    // 401 (auth layer reached) — proves the ready layer doesn't mask auth.
    // The exact code from the auth layer when no cert is presented is 401;
    // we only assert NOT 503 here to keep the contract focused on the gate.
    assert_ne!(
        v1_resp.status(),
        503,
        "ready=true must lift the 503 gate (auth/version layers still apply)",
    );

    server_handle.abort();
}
