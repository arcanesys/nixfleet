//! Integration tests for `/v1/agent/bootstrap-report`.

mod common;

use std::path::PathBuf;

use base64::Engine;
use chrono::{Duration as ChronoDuration, Utc};
use common::{install_crypto_provider_once, pick_free_port, wait_for_listener_ready};
use ed25519_dalek::{Signer, SigningKey};
use nixfleet_control_plane::server;
use nixfleet_proto::enroll_wire::{
    BootstrapEventRequest, BootstrapToken, TokenClaims,
};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use tempfile::TempDir;

fn write(path: &std::path::Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
}

fn mint_fleet_ca(dir: &TempDir) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
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
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .unwrap();

    let ca_cert_path = dir.path().join("ca.pem");
    let ca_key_path = dir.path().join("ca.key");
    let server_cert_path = dir.path().join("server.pem");
    let server_key_path = dir.path().join("server.key");

    write(&ca_cert_path, &ca_cert.pem());
    write(&ca_key_path, &ca_key.serialize_pem());
    write(&server_cert_path, &server_cert.pem());
    write(&server_key_path, &server_key.serialize_pem());

    (ca_cert_path, ca_key_path, server_cert_path, server_key_path)
}

fn write_trust_json(dir: &TempDir, org_root_pubkey_b64: &str) -> PathBuf {
    let path = dir.path().join("trust.json");
    let contents = format!(
        r#"{{
  "schemaVersion": 1,
  "ciReleaseKey": {{ "current": null, "previous": null, "rejectBefore": null }},
  "cacheKeys": [],
  "orgRootKey": {{
    "current": {{ "algorithm": "ed25519", "public": "{org_root_pubkey_b64}" }},
    "previous": null,
    "rejectBefore": null
  }}
}}"#
    );
    write(&path, &contents);
    path
}

fn sign_token(claims: &TokenClaims, signing_key: &SigningKey, version: u32) -> BootstrapToken {
    let claims_json = serde_json::to_string(claims).unwrap();
    let canonical = nixfleet_canonicalize::canonicalize(&claims_json).unwrap();
    let signature = signing_key.sign(canonical.as_bytes());
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
    BootstrapToken {
        version,
        claims: claims.clone(),
        signature: sig_b64,
    }
}

fn random_nonce() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

async fn spawn_server(
    listen: std::net::SocketAddr,
    server_cert: PathBuf,
    server_key: PathBuf,
    fleet_ca_cert: PathBuf,
    fleet_ca_key: PathBuf,
    audit_log: PathBuf,
    obs_dir: &TempDir,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    let artifact = obs_dir.path().join("fleet.resolved.json");
    write(&artifact, "{}");
    let signature = obs_dir.path().join("fleet.resolved.json.sig");
    write(&signature, "");
    // The proper trust.json (orgRootKey populated) is written by each test
    // via write_trust_json at obs_dir/trust.json; ServeArgs.trust_path
    // points at the same file so the bootstrap-report handler reads the
    // operator-configured trust roots, not a stub.
    let trust = obs_dir.path().join("trust.json");
    if !trust.exists() {
        // Default stub for tests that don't call write_trust_json before
        // spawn_server (e.g. the rejects_disallowed_event_variant test
        // hits trust verification before reaching the variant check).
        write(
            &trust,
            r#"{"schemaVersion":1,"ciReleaseKey":{"current":null,"previous":null,"rejectBefore":null},"cacheKeys":[],"orgRootKey":null}"#,
        );
    }
    let observed = obs_dir.path().join("observed.json");
    write(
        &observed,
        r#"{"channelRefs":{},"lastRolledRefs":{},"hostState":{},"activeRollouts":[]}"#,
    );
    let db_path = obs_dir.path().join("state.db");

    let args = server::ServeArgs {
        listen,
        tls_cert: server_cert,
        tls_key: server_key,
        client_ca: None,
        fleet_ca_cert: Some(fleet_ca_cert),
        fleet_ca_key: Some(fleet_ca_key),
        audit_log_path: Some(audit_log),
        artifact_path: artifact,
        signature_path: signature,
        trust_path: trust,
        observed_path: observed,
        confirm_deadline_secs: 120,
        db_path: Some(db_path),
        ..Default::default()
    };
    let port = listen.port();
    let handle = tokio::spawn(server::serve(args));
    wait_for_listener_ready(port, &handle).await;
    handle
}

fn signed_token(signing_key: &SigningKey, hostname: &str) -> BootstrapToken {
    let now = Utc::now();
    let claims = TokenClaims {
        hostname: hostname.to_string(),
        expected_pubkey_fingerprint: "unused-on-this-endpoint".to_string(),
        issued_at: now - ChronoDuration::seconds(5),
        expires_at: now + ChronoDuration::hours(1),
        nonce: random_nonce(),
    };
    sign_token(&claims, signing_key, 1)
}

async fn make_client(ca_cert: &PathBuf) -> reqwest::Client {
    let ca_pem = std::fs::read(ca_cert).unwrap();
    let ca_certb = reqwest::Certificate::from_pem(&ca_pem).unwrap();
    reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(ca_certb)
        .build()
        .unwrap()
}

#[tokio::test]
async fn bootstrap_report_accepts_trust_error() {
    install_crypto_provider_once();
    let dir = TempDir::new().unwrap();
    let (ca_cert, ca_key, server_cert, server_key) = mint_fleet_ca(&dir);
    let audit_log = dir.path().join("issuance.log");

    let mut rng = rand::thread_rng();
    let signing_key = SigningKey::generate(&mut rng);
    let pubkey_b64 = base64::engine::general_purpose::STANDARD
        .encode(signing_key.verifying_key().to_bytes());
    let _trust_path = write_trust_json(&dir, &pubkey_b64);

    let port = pick_free_port().await;
    let listen = format!("127.0.0.1:{port}").parse().unwrap();
    let _handle = spawn_server(
        listen, server_cert, server_key, ca_cert.clone(), ca_key, audit_log, &dir,
    )
    .await;

    let token = signed_token(&signing_key, "test-host");
    let req = BootstrapEventRequest {
        token,
        agent_version: "0.0.0-test".to_string(),
        occurred_at: Utc::now(),
        event: serde_json::json!({
            "event": "trust-error",
            "details": { "reason": "synthetic test" }
        }),
    };
    let client = make_client(&ca_cert).await;
    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/bootstrap-report"))
        .json(&req)
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn bootstrap_report_rejects_disallowed_event_variant() {
    install_crypto_provider_once();
    let dir = TempDir::new().unwrap();
    let (ca_cert, ca_key, server_cert, server_key) = mint_fleet_ca(&dir);
    let audit_log = dir.path().join("issuance.log");

    let mut rng = rand::thread_rng();
    let signing_key = SigningKey::generate(&mut rng);
    let pubkey_b64 = base64::engine::general_purpose::STANDARD
        .encode(signing_key.verifying_key().to_bytes());
    let _trust_path = write_trust_json(&dir, &pubkey_b64);

    let port = pick_free_port().await;
    let listen = format!("127.0.0.1:{port}").parse().unwrap();
    let _handle = spawn_server(
        listen, server_cert, server_key, ca_cert.clone(), ca_key, audit_log, &dir,
    )
    .await;

    let token = signed_token(&signing_key, "test-host");
    let req = BootstrapEventRequest {
        token,
        agent_version: "0.0.0-test".to_string(),
        occurred_at: Utc::now(),
        event: serde_json::json!({
            "event": "rollback-triggered",
            "details": { "reason": "should be rejected via this endpoint" }
        }),
    };
    let client = make_client(&ca_cert).await;
    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/bootstrap-report"))
        .json(&req)
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn bootstrap_report_rejects_bad_signature() {
    install_crypto_provider_once();
    let dir = TempDir::new().unwrap();
    let (ca_cert, ca_key, server_cert, server_key) = mint_fleet_ca(&dir);
    let audit_log = dir.path().join("issuance.log");

    let mut rng = rand::thread_rng();
    let real_key = SigningKey::generate(&mut rng);
    let pubkey_b64 = base64::engine::general_purpose::STANDARD
        .encode(real_key.verifying_key().to_bytes());
    let _trust_path = write_trust_json(&dir, &pubkey_b64);

    let port = pick_free_port().await;
    let listen = format!("127.0.0.1:{port}").parse().unwrap();
    let _handle = spawn_server(
        listen, server_cert, server_key, ca_cert.clone(), ca_key, audit_log, &dir,
    )
    .await;

    // Sign with a DIFFERENT key — server should reject (not in trust.json).
    let attacker_key = SigningKey::generate(&mut rng);
    let token = signed_token(&attacker_key, "test-host");
    let req = BootstrapEventRequest {
        token,
        agent_version: "0.0.0-test".to_string(),
        occurred_at: Utc::now(),
        event: serde_json::json!({
            "event": "trust-error",
            "details": { "reason": "synthetic" }
        }),
    };
    let client = make_client(&ca_cert).await;
    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/bootstrap-report"))
        .json(&req)
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}
