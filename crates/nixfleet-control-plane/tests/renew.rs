//! Integration tests for `/v1/agent/renew`.

mod common;

use std::path::PathBuf;

use chrono::Utc;
use common::{install_crypto_provider_once, pick_free_port, wait_for_listener_ready};
use ed25519_dalek::ed25519::signature::rand_core::{OsRng, RngCore};
use nixfleet_control_plane::{db::Db, server};
use nixfleet_proto::enroll_wire::{RenewRequest, RenewResponse};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, CertificateSigningRequest, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose,
};
use reqwest::Identity;
use tempfile::TempDir;

fn write_pem(path: &std::path::Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
}

struct TestPki {
    ca_cert: PathBuf,
    ca_key: PathBuf,
    server_cert: PathBuf,
    server_key: PathBuf,
    agent_cert_pem: String,
    agent_key_pem: String,
}

fn mint_pki(dir: &TempDir, agent_cn: &str) -> TestPki {
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

    let mut agent_params = CertificateParams::default();
    agent_params
        .distinguished_name
        .push(DnType::CommonName, agent_cn);
    agent_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let agent_key = KeyPair::generate().unwrap();
    let agent_cert = agent_params
        .signed_by(&agent_key, &ca_cert, &ca_key)
        .unwrap();

    let ca_cert_path = dir.path().join("ca.pem");
    let ca_key_path = dir.path().join("ca.key");
    let server_cert_path = dir.path().join("server.pem");
    let server_key_path = dir.path().join("server.key");
    write_pem(&ca_cert_path, &ca_cert.pem());
    write_pem(&ca_key_path, &ca_key.serialize_pem());
    write_pem(&server_cert_path, &server_cert.pem());
    write_pem(&server_key_path, &server_key.serialize_pem());

    TestPki {
        ca_cert: ca_cert_path,
        ca_key: ca_key_path,
        server_cert: server_cert_path,
        server_key: server_key_path,
        agent_cert_pem: agent_cert.pem(),
        agent_key_pem: agent_key.serialize_pem(),
    }
}

/// Write a signed fleet.resolved.json declaring `hostname` with
/// `host_pubkey` so the renew handler's fleet-pubkey binding check
/// passes. Trust.json carries the matching ciReleaseKey for polling
/// loop verification. Returns (artifact, signature, trust, observed).
fn write_signed_fleet_inputs(
    dir: &TempDir,
    hostname: &str,
    host_pubkey: Option<&str>,
) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};

    let mut rng = OsRng;
    let ci_signing_key = SigningKey::generate(&mut rng);
    let public_b64 =
        base64::engine::general_purpose::STANDARD.encode(ci_signing_key.verifying_key());

    let signed_at = "2026-04-26T00:00:00Z";
    let json = serde_json::json!({
        "schemaVersion": 1,
        "hosts": {
            hostname: {
                "system": "x86_64-linux",
                "tags": [],
                "channel": "stable",
                "closureHash": "test-closure",
                "pubkey": host_pubkey,
            }
        },
        "channels": {
            "stable": {
                "rolloutPolicy": "default",
                "reconcileIntervalMinutes": 5,
                "freshnessWindow": 60,
                "signingIntervalMinutes": 30,
                "compliance": { "frameworks": [], "mode": "disabled" },
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
            "signedAt": signed_at,
            "ciCommit": "abc12345deadbeef",
            "signatureAlgorithm": "ed25519",
        },
    });
    let raw = serde_json::to_string(&json).unwrap();
    let canonical = nixfleet_canonicalize::canonicalize(&raw).unwrap();
    let signature = ci_signing_key.sign(canonical.as_bytes());

    let artifact = dir.path().join("fleet.resolved.json");
    std::fs::write(&artifact, &raw).unwrap();
    let signature_path = dir.path().join("fleet.resolved.json.sig");
    std::fs::write(&signature_path, signature.to_bytes()).unwrap();
    // trust.json carries ciReleaseKey for poll-loop verify; orgRootKey
    // is null because renew is mTLS-authenticated (no bootstrap token).
    let trust = dir.path().join("trust.json");
    let trust_json = serde_json::json!({
        "schemaVersion": 1,
        "ciReleaseKey": {
            "current": { "algorithm": "ed25519", "public": public_b64 },
            "previous": null,
            "rejectBefore": null,
        },
        "cacheKeys": [],
        "orgRootKey": null,
    });
    std::fs::write(&trust, trust_json.to_string()).unwrap();

    let observed = dir.path().join("observed.json");
    std::fs::write(
        &observed,
        r#"{"channelRefs":{},"lastRolledRefs":{},"hostState":{},"activeRollouts":[]}"#,
    )
    .unwrap();

    (artifact, signature_path, trust, observed)
}

/// Build the OpenSSH-format pubkey line from a 32-byte ed25519 seed.
fn openssh_pubkey_from_seed(seed: &[u8; 32]) -> String {
    use base64::Engine;
    let dalek_sk = ed25519_dalek::SigningKey::from_bytes(seed);
    let pubkey_raw = dalek_sk.verifying_key().to_bytes();
    let mut blob = Vec::new();
    blob.extend_from_slice(&(b"ssh-ed25519".len() as u32).to_be_bytes());
    blob.extend_from_slice(b"ssh-ed25519");
    blob.extend_from_slice(&(pubkey_raw.len() as u32).to_be_bytes());
    blob.extend_from_slice(&pubkey_raw);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&blob);
    format!("ssh-ed25519 {b64} test@host")
}

/// Wait until the CP has primed its verified_fleet snapshot.
async fn wait_for_fleet_primed(port: u16, ca_pem: &[u8]) {
    let ca_cert = reqwest::Certificate::from_pem(ca_pem).unwrap();
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(ca_cert)
        .build()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if let Ok(r) = client
            .get(format!("https://localhost:{port}/healthz"))
            .send()
            .await
            && r.status().is_success()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("verified_fleet did not prime within 15s");
}

#[allow(clippy::too_many_arguments)]
async fn spawn_server(
    args_dir: &TempDir,
    server_cert: PathBuf,
    server_key: PathBuf,
    client_ca: Option<PathBuf>,
    fleet_ca_cert: Option<PathBuf>,
    fleet_ca_key: Option<PathBuf>,
    db_path: Option<PathBuf>,
    port: u16,
    declared_host: &str,
    host_pubkey: Option<&str>,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    let (artifact, signature, trust, observed) =
        write_signed_fleet_inputs(args_dir, declared_host, host_pubkey);
    let audit_log = args_dir.path().join("issuance.log");
    let listen: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let args = server::ServeArgs {
        listen,
        tls_cert: server_cert,
        tls_key: server_key,
        client_ca,
        fleet_ca_cert,
        fleet_ca_key,
        audit_log_path: Some(audit_log),
        artifact_path: artifact,
        signature_path: signature,
        trust_path: trust,
        observed_path: observed,
        freshness_window: std::time::Duration::from_secs(86400 * 365 * 5),
        confirm_deadline_secs: 120,
        db_path,
        mark_ready_at_startup: true,
        ..Default::default()
    };
    let handle = tokio::spawn(server::serve(args));
    wait_for_listener_ready(port, &handle).await;
    handle
}

fn build_mtls_client(
    ca_pem_path: &std::path::Path,
    agent_pem: &str,
    agent_key_pem: &str,
) -> reqwest::Client {
    let mut combined = agent_pem.as_bytes().to_vec();
    combined.extend_from_slice(agent_key_pem.as_bytes());
    let identity = Identity::from_pem(&combined).unwrap();
    let ca_pem = std::fs::read(ca_pem_path).unwrap();
    let ca_cert = reqwest::Certificate::from_pem(&ca_pem).unwrap();
    reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(ca_cert)
        .identity(identity)
        .build()
        .unwrap()
}

fn build_no_cert_client(ca_pem_path: &std::path::Path) -> reqwest::Client {
    let ca_pem = std::fs::read(ca_pem_path).unwrap();
    let ca_cert = reqwest::Certificate::from_pem(&ca_pem).unwrap();
    reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(ca_cert)
        .build()
        .unwrap()
}

/// Mint a CSR using a caller-supplied 32-byte ed25519 seed (matching
/// the SSH host key the agent would use post #43). Caller declares the
/// matching `pubkey` in fleet.nix; CP renew handler validates the
/// match before issuing the cert.
fn mint_csr_from_seed(hostname: &str, seed: &[u8; 32]) -> String {
    let pkcs8_pem = nixfleet_proto::host_key::ed25519_pkcs8_pem_from_seed(seed);
    let key = KeyPair::from_pem(&pkcs8_pem).unwrap();
    let mut params = CertificateParams::default();
    params.distinguished_name.push(DnType::CommonName, hostname);
    let csr: CertificateSigningRequest = params.serialize_request(&key).unwrap();
    csr.pem().unwrap()
}

fn mint_csr(hostname: &str) -> (String, [u8; 32]) {
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    (mint_csr_from_seed(hostname, &seed), seed)
}

#[tokio::test]
async fn renew_happy_path_signs_fresh_cert() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let pki = mint_pki(&dir, "test-host");
    let (csr_pem, seed) = mint_csr("test-host");
    let openssh = openssh_pubkey_from_seed(&seed);

    let port = pick_free_port().await;
    let handle = spawn_server(
        &dir,
        pki.server_cert.clone(),
        pki.server_key.clone(),
        Some(pki.ca_cert.clone()),
        Some(pki.ca_cert.clone()),
        Some(pki.ca_key.clone()),
        None,
        port,
        "test-host",
        Some(&openssh),
    )
    .await;
    let ca_pem = std::fs::read(&pki.ca_cert).unwrap();
    wait_for_fleet_primed(port, &ca_pem).await;

    let req = RenewRequest { csr_pem };
    let client = build_mtls_client(&pki.ca_cert, &pki.agent_cert_pem, &pki.agent_key_pem);

    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/renew"))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200");
    let body: RenewResponse = resp.json().await.unwrap();
    assert!(
        body.cert_pem.starts_with("-----BEGIN CERTIFICATE-----"),
        "renew should return a PEM-encoded cert; got: {}",
        body.cert_pem.chars().take(40).collect::<String>(),
    );
    assert!(
        body.not_after > Utc::now(),
        "not_after must be in the future",
    );

    handle.abort();
}

#[tokio::test]
async fn renew_rejects_request_without_client_cert() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let pki = mint_pki(&dir, "test-host");
    let (csr_pem, seed) = mint_csr("test-host");
    let openssh = openssh_pubkey_from_seed(&seed);

    let port = pick_free_port().await;
    let handle = spawn_server(
        &dir,
        pki.server_cert.clone(),
        pki.server_key.clone(),
        Some(pki.ca_cert.clone()),
        Some(pki.ca_cert.clone()),
        Some(pki.ca_key.clone()),
        None,
        port,
        "test-host",
        Some(&openssh),
    )
    .await;

    let req = RenewRequest { csr_pem };
    let client = build_no_cert_client(&pki.ca_cert);

    // GOTCHA: TLS-layer rejection or HTTP 401 are both acceptable shapes for "rejected".
    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/renew"))
        .json(&req)
        .send()
        .await;
    if let Ok(r) = resp {
        assert_eq!(r.status(), 401, "expected 401, got {}", r.status());
    }

    handle.abort();
}

#[tokio::test]
async fn renew_rejects_revoked_cert() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let pki = mint_pki(&dir, "test-host");
    let db_path = dir.path().join("state.db");
    let (csr_pem, seed) = mint_csr("test-host");
    let openssh = openssh_pubkey_from_seed(&seed);

    {
        let db = Db::open(&db_path).unwrap();
        db.migrate().unwrap();
        let revoked_before = Utc::now() + chrono::Duration::days(365);
        db.revocations()
            .revoke_cert(
                "test-host",
                revoked_before,
                Some("test"),
                Some("test-operator"),
            )
            .unwrap();
    }

    let port = pick_free_port().await;
    let handle = spawn_server(
        &dir,
        pki.server_cert.clone(),
        pki.server_key.clone(),
        Some(pki.ca_cert.clone()),
        Some(pki.ca_cert.clone()),
        Some(pki.ca_key.clone()),
        Some(db_path),
        port,
        "test-host",
        Some(&openssh),
    )
    .await;

    let req = RenewRequest { csr_pem };
    let client = build_mtls_client(&pki.ca_cert, &pki.agent_cert_pem, &pki.agent_key_pem);

    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/renew"))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "revoked cert must not be able to renew",);

    handle.abort();
}

/// Reproduces the production-realistic case: cert CN is the canonical
/// `agent-<machineId>.<suffix>` form (what the issuance CA actually
/// produces), revocation row stores the short hostname (what the
/// operator writes in fleet.nix). Pre-fix the middleware looked up the
/// full CN against the short DB key and never matched - revocations
/// were silently inert at runtime. Surfaced on hardware testing
/// 2026-05-13 with aether.
#[tokio::test]
async fn renew_rejects_revoked_cert_canonical_cn() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let suffix = nixfleet_control_plane::auth::issuance::DEFAULT_AGENT_CN_SUFFIX;
    let canonical_cn = format!("agent-test-host.{suffix}");
    let pki = mint_pki(&dir, &canonical_cn);
    let db_path = dir.path().join("state.db");
    let (csr_pem, seed) = mint_csr("test-host");
    let openssh = openssh_pubkey_from_seed(&seed);

    {
        let db = Db::open(&db_path).unwrap();
        db.migrate().unwrap();
        // Operator writes the short hostname in fleet.nix - revocations
        // poll replays that form into cert_revocations.hostname.
        let revoked_before = Utc::now() + chrono::Duration::days(365);
        db.revocations()
            .revoke_cert(
                "test-host",
                revoked_before,
                Some("test"),
                Some("test-operator"),
            )
            .unwrap();
    }

    let port = pick_free_port().await;
    let handle = spawn_server(
        &dir,
        pki.server_cert.clone(),
        pki.server_key.clone(),
        Some(pki.ca_cert.clone()),
        Some(pki.ca_cert.clone()),
        Some(pki.ca_key.clone()),
        Some(db_path),
        port,
        "test-host",
        Some(&openssh),
    )
    .await;

    let req = RenewRequest { csr_pem };
    let client = build_mtls_client(&pki.ca_cert, &pki.agent_cert_pem, &pki.agent_key_pem);

    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/renew"))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "revoked cert with canonical CN must not be able to renew",
    );

    handle.abort();
}

#[tokio::test]
async fn renew_returns_500_when_ca_not_configured() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let pki = mint_pki(&dir, "test-host");
    let (csr_pem, seed) = mint_csr("test-host");
    let openssh = openssh_pubkey_from_seed(&seed);

    let port = pick_free_port().await;
    let handle = spawn_server(
        &dir,
        pki.server_cert.clone(),
        pki.server_key.clone(),
        Some(pki.ca_cert.clone()),
        None,
        None,
        None,
        port,
        "test-host",
        Some(&openssh),
    )
    .await;
    let ca_pem = std::fs::read(&pki.ca_cert).unwrap();
    wait_for_fleet_primed(port, &ca_pem).await;

    let req = RenewRequest { csr_pem };
    let client = build_mtls_client(&pki.ca_cert, &pki.agent_cert_pem, &pki.agent_key_pem);

    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/renew"))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        500,
        "no CA configured must surface 500, not silent success",
    );

    handle.abort();
}

/// RFC-0003 §2 binding at the renewal seam: an mTLS-authenticated
/// agent that submits a CSR signed by a different keypair than the
/// host's declared SSH host key is rejected. Without this, an attacker
/// with control of the agent process (but not the SSH host key) could
/// silently rotate the cert binding to a fresh keypair, breaking the
/// "compromise of cert ⇔ compromise of host key" equivalence the RFC
/// promises.
#[tokio::test]
async fn renew_rejects_csr_pubkey_mismatch_with_declared_host() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let pki = mint_pki(&dir, "test-host");

    let mut declared_seed = [0u8; 32];
    OsRng.fill_bytes(&mut declared_seed);
    let declared_openssh = openssh_pubkey_from_seed(&declared_seed);

    // CSR signed with a DIFFERENT seed than what fleet.nix declares.
    let mut imposter_seed = [0u8; 32];
    OsRng.fill_bytes(&mut imposter_seed);
    assert_ne!(declared_seed, imposter_seed);
    let csr_pem = mint_csr_from_seed("test-host", &imposter_seed);

    let port = pick_free_port().await;
    let handle = spawn_server(
        &dir,
        pki.server_cert.clone(),
        pki.server_key.clone(),
        Some(pki.ca_cert.clone()),
        Some(pki.ca_cert.clone()),
        Some(pki.ca_key.clone()),
        None,
        port,
        "test-host",
        Some(&declared_openssh),
    )
    .await;
    let ca_pem = std::fs::read(&pki.ca_cert).unwrap();
    wait_for_fleet_primed(port, &ca_pem).await;

    let req = RenewRequest { csr_pem };
    let client = build_mtls_client(&pki.ca_cert, &pki.agent_cert_pem, &pki.agent_key_pem);

    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/renew"))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "CSR-vs-declared mismatch must reject (RFC-0003 §2 binding)",
    );

    handle.abort();
}
