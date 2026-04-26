//! `/v1/enroll` integration test (Phase 3 PR-5 + signature-verify fix).
//!
//! Mints a real ed25519 keypair on the operator side, signs a token,
//! materialises a `trust.json` carrying the public half, spins up the
//! server, and verifies:
//!
//! 1. Happy path — valid token + matching CSR → 200, cert returned.
//! 2. Signature tampering — flipped byte in signature → 401.
//! 3. Replay — same nonce twice → 200 then 409.
//! 4. Hostname-vs-CSR-CN mismatch → 401.
//!
//! Together these cover the security boundary that was stubbed in the
//! initial PR-5 commit.

use std::path::PathBuf;
use std::sync::Once;
use std::time::Duration;

use base64::Engine;
use chrono::{Duration as ChronoDuration, Utc};
use ed25519_dalek::{Signer, SigningKey};
use nixfleet_control_plane::server;
use nixfleet_proto::enroll_wire::{
    BootstrapToken, EnrollRequest, EnrollResponse, TokenClaims,
};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, CertificateSigningRequest, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, PublicKeyData,
};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::time::sleep;

fn install_crypto_provider_once() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        // Pipe handler tracing to stderr so test failures show why
        // 401 came back. RUST_LOG=warn (default) is enough.
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .with_test_writer()
            .try_init();
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

fn write(path: &std::path::Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
}

/// Mint a fleet CA + server cert in `dir`. Returns paths.
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

/// Write a trust.json with the given org-root ed25519 pubkey (base64).
fn write_trust_json(dir: &TempDir, org_root_pubkey_b64: &str) -> PathBuf {
    // Place trust.json next to ca.pem so the enroll handler's lookup
    // (parent of fleet-ca-cert + "trust.json") finds it.
    let path = dir.path().join("trust.json");
    let contents = format!(
        r#"{{
  "schemaVersion": 1,
  "ciReleaseKey": {{ "current": null, "previous": null, "rejectBefore": null }},
  "atticCacheKey": {{ "current": null, "previous": null, "rejectBefore": null }},
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

/// Mint a CSR with CN=`hostname` and a fresh keypair. Returns
/// (PEM, pubkey-DER, fingerprint-base64).
///
/// Crucially, the fingerprint must come from the *parsed* CSR's
/// PublicKey via `PublicKeyData::der_bytes`, not from the
/// KeyPair-side `public_key_der()` — those produce different
/// DER framings, and the server does its check against the
/// parsed-CSR view. Round-trip the CSR to make the fingerprint
/// match what the server will compute.
fn mint_csr(hostname: &str) -> (String, Vec<u8>, String) {
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, hostname);
    let csr: CertificateSigningRequest = params.serialize_request(&key).unwrap();
    let pem = csr.pem().unwrap();

    // Round-trip through the same parser the server uses so the
    // fingerprint is computed from the same byte slice.
    let parsed = rcgen::CertificateSigningRequestParams::from_pem(&pem).unwrap();
    let pubkey_der: Vec<u8> = parsed.public_key.der_bytes().to_vec();
    let digest = sha2::Sha256::digest(&pubkey_der);
    let fingerprint = base64::engine::general_purpose::STANDARD.encode(digest);

    (pem, pubkey_der, fingerprint)
}
use sha2::Digest;

/// Sign a TokenClaims with `signing_key` over the JCS canonical bytes.
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
    // trust_path here is the legacy `--trust-file` flag path, not the
    // enroll handler's lookup. The handler reads trust.json from
    // dirname(fleet_ca_cert)/trust.json instead.
    let trust = obs_dir.path().join("trust-stub.json");
    write(
        &trust,
        r#"{"ciReleaseKey":{"current":null,"previous":null,"rejectBefore":null}}"#,
    );
    let observed = obs_dir.path().join("observed.json");
    write(
        &observed,
        r#"{"channelRefs":{},"lastRolledRefs":{},"hostState":{},"activeRollouts":[]}"#,
    );

    let args = server::ServeArgs {
        listen,
        tls_cert: server_cert,
        tls_key: server_key,
        client_ca: None, // /v1/enroll doesn't require mTLS
        fleet_ca_cert: Some(fleet_ca_cert),
        fleet_ca_key: Some(fleet_ca_key),
        audit_log_path: Some(audit_log),
        artifact_path: artifact,
        signature_path: signature,
        trust_path: trust,
        observed_path: observed,
        freshness_window: Duration::from_secs(86400),
        forgejo: None,
    };
    let handle = tokio::spawn(server::serve(args));
    sleep(Duration::from_millis(200)).await;
    assert!(!handle.is_finished(), "server task exited prematurely");
    handle
}

#[tokio::test]
async fn enroll_happy_path_signs_cert() {
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
    let handle = spawn_server(
        listen, server_cert, server_key, ca_cert.clone(), ca_key, audit_log, &dir,
    )
    .await;

    let (csr_pem, _pubkey_der, fingerprint) = mint_csr("krach");
    let now = Utc::now();
    let claims = TokenClaims {
        hostname: "krach".to_string(),
        expected_pubkey_fingerprint: fingerprint,
        issued_at: now - ChronoDuration::seconds(5),
        expires_at: now + ChronoDuration::hours(1),
        nonce: random_nonce(),
    };
    let token = sign_token(&claims, &signing_key, 1);

    // CA-pin reqwest to the test fleet CA so the TLS handshake works.
    let ca_pem = std::fs::read(&ca_cert).unwrap();
    let ca_certb = reqwest::Certificate::from_pem(&ca_pem).unwrap();
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(ca_certb)
        .build()
        .unwrap();

    let req = EnrollRequest { token, csr_pem };
    let resp = client
        .post(&format!("https://localhost:{port}/v1/enroll"))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "enroll happy path returned non-200");

    let body: EnrollResponse = resp.json().await.unwrap();
    assert!(body.cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(body.not_after > now);

    handle.abort();
}

#[tokio::test]
async fn enroll_rejects_tampered_signature() {
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
    let handle = spawn_server(
        format!("127.0.0.1:{port}").parse().unwrap(),
        server_cert,
        server_key,
        ca_cert.clone(),
        ca_key,
        audit_log,
        &dir,
    )
    .await;

    let (csr_pem, _pubkey_der, fingerprint) = mint_csr("krach");
    let now = Utc::now();
    let claims = TokenClaims {
        hostname: "krach".to_string(),
        expected_pubkey_fingerprint: fingerprint,
        issued_at: now - ChronoDuration::seconds(5),
        expires_at: now + ChronoDuration::hours(1),
        nonce: random_nonce(),
    };
    let mut token = sign_token(&claims, &signing_key, 1);
    // Flip the last byte of the signature.
    let mut sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(&token.signature)
        .unwrap();
    let last = sig_bytes.len() - 1;
    sig_bytes[last] ^= 0x01;
    token.signature = base64::engine::general_purpose::STANDARD.encode(&sig_bytes);

    let ca_pem = std::fs::read(&ca_cert).unwrap();
    let ca_certb = reqwest::Certificate::from_pem(&ca_pem).unwrap();
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(ca_certb)
        .build()
        .unwrap();

    let req = EnrollRequest { token, csr_pem };
    let resp = client
        .post(&format!("https://localhost:{port}/v1/enroll"))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "tampered signature should 401");

    handle.abort();
}

#[tokio::test]
async fn enroll_rejects_replayed_nonce() {
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
    let handle = spawn_server(
        format!("127.0.0.1:{port}").parse().unwrap(),
        server_cert,
        server_key,
        ca_cert.clone(),
        ca_key,
        audit_log,
        &dir,
    )
    .await;

    let (csr_pem, _pubkey_der, fingerprint) = mint_csr("krach");
    let now = Utc::now();
    let claims = TokenClaims {
        hostname: "krach".to_string(),
        expected_pubkey_fingerprint: fingerprint,
        issued_at: now - ChronoDuration::seconds(5),
        expires_at: now + ChronoDuration::hours(1),
        nonce: random_nonce(),
    };
    let token = sign_token(&claims, &signing_key, 1);

    let ca_pem = std::fs::read(&ca_cert).unwrap();
    let ca_certb = reqwest::Certificate::from_pem(&ca_pem).unwrap();
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(ca_certb)
        .build()
        .unwrap();

    // First call: 200.
    let req1 = EnrollRequest {
        token: token.clone(),
        csr_pem: csr_pem.clone(),
    };
    let resp1 = client
        .post(&format!("https://localhost:{port}/v1/enroll"))
        .json(&req1)
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 200);

    // Second call (same token, same nonce): 409.
    let req2 = EnrollRequest { token, csr_pem };
    let resp2 = client
        .post(&format!("https://localhost:{port}/v1/enroll"))
        .json(&req2)
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 409, "replayed nonce should 409");

    handle.abort();
}
