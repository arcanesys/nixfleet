//! Integration tests for `/v1/enroll`.

mod common;

use std::path::PathBuf;

use base64::Engine;
use chrono::{Duration as ChronoDuration, Utc};
use common::{install_crypto_provider_once, pick_free_port, wait_for_listener_ready};
use ed25519_dalek::{Signer, SigningKey};
use nixfleet_control_plane::server;
use nixfleet_proto::enroll_wire::{BootstrapToken, EnrollRequest, EnrollResponse, TokenClaims};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, CertificateSigningRequest, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, PublicKeyData,
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

    let mut server_params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
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

/// Mint a CSR using a caller-supplied 32-byte ed25519 seed (= the
/// "SSH host key" the agent would use post #43). Returns `(pem,
/// pubkey_der, fingerprint, seed)`. The seed is returned so the
/// caller can declare the matching pubkey in the fleet artifact.
fn mint_csr_with_seed(hostname: &str, seed: &[u8; 32]) -> (String, Vec<u8>, String, [u8; 32]) {
    let pkcs8_pem = nixfleet_proto::host_key::ed25519_pkcs8_pem_from_seed(seed);
    let key = KeyPair::from_pem(&pkcs8_pem).unwrap();
    let mut params = CertificateParams::default();
    params.distinguished_name.push(DnType::CommonName, hostname);
    let csr: CertificateSigningRequest = params.serialize_request(&key).unwrap();
    let pem = csr.pem().unwrap();

    let parsed = rcgen::CertificateSigningRequestParams::from_pem(&pem).unwrap();
    let pubkey_der: Vec<u8> = parsed.public_key.der_bytes().to_vec();
    let digest = sha2::Sha256::digest(&pubkey_der);
    let fingerprint = base64::engine::general_purpose::STANDARD.encode(digest);

    (pem, pubkey_der, fingerprint, *seed)
}

/// Build the OpenSSH-format pubkey line (`ssh-ed25519 <b64> test@host`)
/// from a 32-byte ed25519 raw pubkey. Same wire shape `fleet.nix` carries.
fn openssh_pubkey_line(raw: &[u8; 32]) -> String {
    let mut blob = Vec::new();
    blob.extend_from_slice(&(b"ssh-ed25519".len() as u32).to_be_bytes());
    blob.extend_from_slice(b"ssh-ed25519");
    blob.extend_from_slice(&(raw.len() as u32).to_be_bytes());
    blob.extend_from_slice(raw);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&blob);
    format!("ssh-ed25519 {b64} test@host")
}

/// Derive the OpenSSH pubkey line from a 32-byte ed25519 seed.
fn openssh_pubkey_from_seed(seed: &[u8; 32]) -> String {
    let dalek_sk = ed25519_dalek::SigningKey::from_bytes(seed);
    let pubkey_raw = dalek_sk.verifying_key().to_bytes();
    openssh_pubkey_line(&pubkey_raw)
}

/// Write a signed fleet.resolved.json declaring `hostname` with the
/// supplied OpenSSH pubkey + a trust.json carrying BOTH the
/// `ciReleaseKey` (for fleet artifact verification by the polling
/// loop) AND the `orgRootKey` provided in `org_root_pubkey_b64` (for
/// bootstrap-token verification by `/v1/enroll`).
///
/// Returns `(artifact, signature, trust)` paths. The single trust.json
/// is BOTH `args.trust_path` for the polling loop AND
/// `dirname(fleet_ca_cert)/trust.json` for the enroll handler - same
/// file, two readers.
fn write_signed_fleet_with_host(
    dir: &TempDir,
    hostname: &str,
    host_pubkey: Option<&str>,
    org_root_pubkey_b64: &str,
) -> (PathBuf, PathBuf, PathBuf) {
    let mut rng = rand::thread_rng();
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
    let trust = dir.path().join("trust.json");
    let trust_json = serde_json::json!({
        "schemaVersion": 1,
        "ciReleaseKey": {
            "current": { "algorithm": "ed25519", "public": public_b64 },
            "previous": null,
            "rejectBefore": null,
        },
        "cacheKeys": [],
        "orgRootKey": {
            "current": { "algorithm": "ed25519", "public": org_root_pubkey_b64 },
            "previous": null,
            "rejectBefore": null,
        },
    });
    std::fs::write(&trust, trust_json.to_string()).unwrap();
    (artifact, signature_path, trust)
}

/// Wait until the CP has primed its verified_fleet snapshot. Polls
/// `/healthz` (which becomes 200 only after first verify tick lands).
/// Without this, enroll tests race the polling loop and get 503.
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
        {
            if r.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("verified_fleet did not prime within 15s");
}
use sha2::Digest;

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

#[allow(clippy::too_many_arguments)]
async fn spawn_server(
    listen: std::net::SocketAddr,
    server_cert: PathBuf,
    server_key: PathBuf,
    fleet_ca_cert: PathBuf,
    fleet_ca_key: PathBuf,
    audit_log: PathBuf,
    artifact: PathBuf,
    signature: PathBuf,
    trust: PathBuf,
    obs_dir: &TempDir,
    initial_nonces: Option<nixfleet_control_plane::db::allowed_nonces::AllowedNoncesView>,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
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
        freshness_window: std::time::Duration::from_secs(86400 * 365 * 5),
        confirm_deadline_secs: 120,
        db_path: Some(db_path),
        mark_ready_at_startup: true,
        initial_nonces,
        ..Default::default()
    };
    let port = listen.port();
    let handle = tokio::spawn(server::serve(args));
    wait_for_listener_ready(port, &handle).await;
    handle
}

/// Build an `AllowedNoncesView` containing a single live entry for
/// `(nonce, hostname)`. Used by success-path enroll tests to satisfy
/// the bootstrap-nonce allowlist guard without a running poll loop.
fn single_entry_nonces_view(
    nonce: &str,
    hostname: &str,
) -> nixfleet_control_plane::db::allowed_nonces::AllowedNoncesView {
    nixfleet_control_plane::db::allowed_nonces::AllowedNoncesView::from_artifact(
        nixfleet_proto::BootstrapNonces {
            schema_version: 1,
            bootstrap_nonces: vec![nixfleet_proto::BootstrapNonceEntry {
                nonce: nonce.to_string(),
                hostname: hostname.to_string(),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                minted_at: None,
                minted_by: None,
            }],
            meta: nixfleet_proto::Meta {
                schema_version: 1,
                signed_at: Some(chrono::Utc::now()),
                ci_commit: None,
                signature_algorithm: Some("ecdsa-p256".into()),
            },
        },
    )
}

/// End-to-end test setup: mint CA + server cert, mint a "host SSH host
/// key" seed, sign a fleet artifact declaring `test-host` with the
/// matching OpenSSH pubkey, spawn the CP, wait for fleet to prime.
/// Returns everything tests need to mint tokens + CSRs.
struct EnrollHarness {
    handle: tokio::task::JoinHandle<anyhow::Result<()>>,
    port: u16,
    ca_cert: PathBuf,
    org_root_signing_key: SigningKey,
}

async fn setup_enroll_harness_with_declared_host(
    declared_host: &str,
    host_pubkey_in_fleet: Option<&str>,
    initial_nonces: Option<nixfleet_control_plane::db::allowed_nonces::AllowedNoncesView>,
) -> (TempDir, EnrollHarness) {
    install_crypto_provider_once();
    let dir = TempDir::new().unwrap();
    let (ca_cert, ca_key, server_cert, server_key) = mint_fleet_ca(&dir);
    let audit_log = dir.path().join("issuance.log");

    let mut rng = rand::thread_rng();
    let org_root_signing_key = SigningKey::generate(&mut rng);
    let org_root_pubkey_b64 = base64::engine::general_purpose::STANDARD
        .encode(org_root_signing_key.verifying_key().to_bytes());

    let (artifact, signature, trust) = write_signed_fleet_with_host(
        &dir,
        declared_host,
        host_pubkey_in_fleet,
        &org_root_pubkey_b64,
    );

    let port = pick_free_port().await;
    let handle = spawn_server(
        format!("127.0.0.1:{port}").parse().unwrap(),
        server_cert,
        server_key,
        ca_cert.clone(),
        ca_key,
        audit_log,
        artifact,
        signature,
        trust,
        &dir,
        initial_nonces,
    )
    .await;

    let ca_pem = std::fs::read(&ca_cert).unwrap();
    wait_for_fleet_primed(port, &ca_pem).await;

    (
        dir,
        EnrollHarness {
            handle,
            port,
            ca_cert,
            org_root_signing_key,
        },
    )
}

fn build_enroll_client(ca_cert: &std::path::Path) -> reqwest::Client {
    let ca_pem = std::fs::read(ca_cert).unwrap();
    let ca_certb = reqwest::Certificate::from_pem(&ca_pem).unwrap();
    reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(ca_certb)
        .build()
        .unwrap()
}

#[tokio::test]
async fn enroll_happy_path_signs_cert() {
    let mut h_seed = [0u8; 32];
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(&mut h_seed);
    let openssh = openssh_pubkey_from_seed(&h_seed);

    // Pre-generate the nonce so it can be seeded into the allowlist before spawn.
    let nonce = random_nonce();
    let initial_nonces = single_entry_nonces_view(&nonce, "test-host");

    let (_dir, harness) =
        setup_enroll_harness_with_declared_host("test-host", Some(&openssh), Some(initial_nonces))
            .await;

    let (csr_pem, _pubkey_der, fingerprint, _) = mint_csr_with_seed("test-host", &h_seed);
    let now = Utc::now();
    let claims = TokenClaims {
        hostname: "test-host".to_string(),
        expected_pubkey_fingerprint: fingerprint,
        issued_at: now - ChronoDuration::seconds(5),
        expires_at: now + ChronoDuration::hours(1),
        nonce,
    };
    let token = sign_token(&claims, &harness.org_root_signing_key, 1);

    let client = build_enroll_client(&harness.ca_cert);
    let req = EnrollRequest { token, csr_pem };
    let resp = client
        .post(format!("https://localhost:{}/v1/enroll", harness.port))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "enroll happy path returned non-200");

    let body: EnrollResponse = resp.json().await.unwrap();
    assert!(body.cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(body.not_after > now);

    harness.handle.abort();
}

#[tokio::test]
async fn enroll_rejects_tampered_signature() {
    use rand::RngCore;
    let mut h_seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut h_seed);
    let openssh = openssh_pubkey_from_seed(&h_seed);
    // Pre-generate the nonce and seed the allowlist so the request reaches
    // verify_bootstrap_token_against_trust (signature verification path),
    // not nonce_not_allowlisted.
    let nonce = random_nonce();
    let initial_nonces = single_entry_nonces_view(&nonce, "test-host");
    let (_dir, harness) =
        setup_enroll_harness_with_declared_host("test-host", Some(&openssh), Some(initial_nonces))
            .await;

    let (csr_pem, _pubkey_der, fingerprint, _) = mint_csr_with_seed("test-host", &h_seed);
    let now = Utc::now();
    let claims = TokenClaims {
        hostname: "test-host".to_string(),
        expected_pubkey_fingerprint: fingerprint,
        issued_at: now - ChronoDuration::seconds(5),
        expires_at: now + ChronoDuration::hours(1),
        nonce,
    };
    let mut token = sign_token(&claims, &harness.org_root_signing_key, 1);
    let mut sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(&token.signature)
        .unwrap();
    let last = sig_bytes.len() - 1;
    sig_bytes[last] ^= 0x01;
    token.signature = base64::engine::general_purpose::STANDARD.encode(&sig_bytes);

    let client = build_enroll_client(&harness.ca_cert);
    let req = EnrollRequest { token, csr_pem };
    let resp = client
        .post(format!("https://localhost:{}/v1/enroll", harness.port))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "tampered signature should 401");

    harness.handle.abort();
}

#[tokio::test]
async fn enroll_rejects_replayed_nonce() {
    use rand::RngCore;
    let mut h_seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut h_seed);
    let openssh = openssh_pubkey_from_seed(&h_seed);

    // Pre-generate the nonce so it can be seeded into the allowlist before spawn.
    // The second (replayed) request hits the DB nonce-replay guard (409), not
    // the allowlist guard, since the nonce IS in the allowlist.
    let nonce = random_nonce();
    let initial_nonces = single_entry_nonces_view(&nonce, "test-host");

    let (_dir, harness) =
        setup_enroll_harness_with_declared_host("test-host", Some(&openssh), Some(initial_nonces))
            .await;

    let (csr_pem, _pubkey_der, fingerprint, _) = mint_csr_with_seed("test-host", &h_seed);
    let now = Utc::now();
    let claims = TokenClaims {
        hostname: "test-host".to_string(),
        expected_pubkey_fingerprint: fingerprint,
        issued_at: now - ChronoDuration::seconds(5),
        expires_at: now + ChronoDuration::hours(1),
        nonce,
    };
    let token = sign_token(&claims, &harness.org_root_signing_key, 1);

    let client = build_enroll_client(&harness.ca_cert);
    let req1 = EnrollRequest {
        token: token.clone(),
        csr_pem: csr_pem.clone(),
    };
    let resp1 = client
        .post(format!("https://localhost:{}/v1/enroll", harness.port))
        .json(&req1)
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 200);

    let req2 = EnrollRequest { token, csr_pem };
    let resp2 = client
        .post(format!("https://localhost:{}/v1/enroll", harness.port))
        .json(&req2)
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 409, "replayed nonce should 409");

    harness.handle.abort();
}

/// CSR pubkey doesn't match the host's declared SSH pubkey in
/// fleet.nix. Closes RFC-0003 §2: even with a valid bootstrap token,
/// the agent must produce a CSR signed by the SSH host key the
/// operator already declared - replay/swap of a leaked-token-elsewhere
/// can't enrol with a different keypair.
#[tokio::test]
async fn enroll_rejects_csr_pubkey_mismatch_with_declared_host() {
    use rand::RngCore;
    let mut declared_seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut declared_seed);
    let declared_openssh = openssh_pubkey_from_seed(&declared_seed);
    // Pre-generate the nonce and seed the allowlist so the request reaches the
    // CSR pubkey binding check (RFC-0003 §2), not nonce_not_allowlisted.
    let nonce = random_nonce();
    let initial_nonces = single_entry_nonces_view(&nonce, "test-host");
    let (_dir, harness) =
        setup_enroll_harness_with_declared_host("test-host", Some(&declared_openssh), Some(initial_nonces))
            .await;

    // CSR signed with a DIFFERENT seed than what fleet.nix declares.
    let mut imposter_seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut imposter_seed);
    assert_ne!(
        declared_seed, imposter_seed,
        "test setup: seeds must differ"
    );
    let (csr_pem, _, fingerprint, _) = mint_csr_with_seed("test-host", &imposter_seed);

    let now = Utc::now();
    let claims = TokenClaims {
        hostname: "test-host".to_string(),
        expected_pubkey_fingerprint: fingerprint,
        issued_at: now - ChronoDuration::seconds(5),
        expires_at: now + ChronoDuration::hours(1),
        nonce,
    };
    let token = sign_token(&claims, &harness.org_root_signing_key, 1);

    let client = build_enroll_client(&harness.ca_cert);
    let req = EnrollRequest { token, csr_pem };
    let resp = client
        .post(format!("https://localhost:{}/v1/enroll", harness.port))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "CSR-vs-declared mismatch must reject (RFC-0003 §2 binding)",
    );

    harness.handle.abort();
}

/// Build an `AllowedNoncesView` that pairs `nonce` with `allowlist_hostname`,
/// regardless of what hostname the token will claim. Used to test the
/// `nonce_hostname_mismatch` rejection path.
fn nonces_view_with_hostname(
    nonce: &str,
    allowlist_hostname: &str,
) -> nixfleet_control_plane::db::allowed_nonces::AllowedNoncesView {
    nixfleet_control_plane::db::allowed_nonces::AllowedNoncesView::from_artifact(
        nixfleet_proto::BootstrapNonces {
            schema_version: 1,
            bootstrap_nonces: vec![nixfleet_proto::BootstrapNonceEntry {
                nonce: nonce.to_string(),
                hostname: allowlist_hostname.to_string(),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                minted_at: None,
                minted_by: None,
            }],
            meta: nixfleet_proto::Meta {
                schema_version: 1,
                signed_at: Some(chrono::Utc::now()),
                ci_commit: None,
                signature_algorithm: Some("ecdsa-p256".into()),
            },
        },
    )
}

/// Build an `AllowedNoncesView` with an already-expired entry for
/// `(nonce, hostname)`. Used to test the `nonce_allowlist_expired` path.
fn expired_nonces_view(
    nonce: &str,
    hostname: &str,
) -> nixfleet_control_plane::db::allowed_nonces::AllowedNoncesView {
    nixfleet_control_plane::db::allowed_nonces::AllowedNoncesView::from_artifact(
        nixfleet_proto::BootstrapNonces {
            schema_version: 1,
            bootstrap_nonces: vec![nixfleet_proto::BootstrapNonceEntry {
                nonce: nonce.to_string(),
                hostname: hostname.to_string(),
                // Expired 1 hour ago.
                expires_at: chrono::Utc::now() - chrono::Duration::hours(1),
                minted_at: None,
                minted_by: None,
            }],
            meta: nixfleet_proto::Meta {
                schema_version: 1,
                signed_at: Some(chrono::Utc::now()),
                ci_commit: None,
                signature_algorithm: Some("ecdsa-p256".into()),
            },
        },
    )
}

/// Nonce absent from the signed allowlist: enrollment must be rejected (401).
/// This is the primary post-fix invariant for nixfleet#96 - a nonce that was
/// never minted (or was pruned after use) cannot be consumed.
#[tokio::test]
async fn enroll_rejects_when_nonce_not_in_allowlist() {
    use rand::RngCore;
    let mut h_seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut h_seed);
    let openssh = openssh_pubkey_from_seed(&h_seed);

    // Spawn with an EMPTY allowlist (None -> default empty view).
    let (_dir, harness) =
        setup_enroll_harness_with_declared_host("test-host", Some(&openssh), None).await;

    let (csr_pem, _pubkey_der, fingerprint, _) = mint_csr_with_seed("test-host", &h_seed);
    let now = Utc::now();
    let claims = TokenClaims {
        hostname: "test-host".to_string(),
        expected_pubkey_fingerprint: fingerprint,
        issued_at: now - ChronoDuration::seconds(5),
        expires_at: now + ChronoDuration::hours(1),
        // Any nonce that was never added to the allowlist.
        nonce: random_nonce(),
    };
    let token = sign_token(&claims, &harness.org_root_signing_key, 1);

    let client = build_enroll_client(&harness.ca_cert);
    let req = EnrollRequest { token, csr_pem };
    let resp = client
        .post(format!("https://localhost:{}/v1/enroll", harness.port))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "nonce absent from allowlist must reject (nonce_not_allowlisted)",
    );

    harness.handle.abort();
}

/// Nonce is in the allowlist but paired with a DIFFERENT hostname than the
/// token claims. Enrollment must be rejected (401).
/// Guards against minting a token for host-a and trying to enrol host-b.
#[tokio::test]
async fn enroll_rejects_when_allowlist_hostname_mismatches_token() {
    use rand::RngCore;
    let mut h_seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut h_seed);
    let openssh = openssh_pubkey_from_seed(&h_seed);

    let nonce = random_nonce();
    // Allowlist entry says nonce belongs to "other-host", not "test-host".
    let initial_nonces = nonces_view_with_hostname(&nonce, "other-host");

    // Fleet still declares "test-host" so the fleet-lookup check would pass
    // if the allowlist check were absent. The allowlist mismatch must fire first.
    let (_dir, harness) =
        setup_enroll_harness_with_declared_host("test-host", Some(&openssh), Some(initial_nonces))
            .await;

    let (csr_pem, _pubkey_der, fingerprint, _) = mint_csr_with_seed("test-host", &h_seed);
    let now = Utc::now();
    let claims = TokenClaims {
        hostname: "test-host".to_string(),
        expected_pubkey_fingerprint: fingerprint,
        issued_at: now - ChronoDuration::seconds(5),
        expires_at: now + ChronoDuration::hours(1),
        nonce,
    };
    let token = sign_token(&claims, &harness.org_root_signing_key, 1);

    let client = build_enroll_client(&harness.ca_cert);
    let req = EnrollRequest { token, csr_pem };
    let resp = client
        .post(format!("https://localhost:{}/v1/enroll", harness.port))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "allowlist hostname mismatch must reject (nonce_hostname_mismatch)",
    );

    harness.handle.abort();
}

/// Allowlist entry for the nonce exists and hostname matches, but the entry
/// has already expired. Enrollment must be rejected (401).
/// This is defense-in-depth: the release tool prunes expired entries at sign
/// time, but this check closes the clock-skew window.
#[tokio::test]
async fn enroll_rejects_when_allowlist_entry_expired() {
    use rand::RngCore;
    let mut h_seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut h_seed);
    let openssh = openssh_pubkey_from_seed(&h_seed);

    let nonce = random_nonce();
    // Entry has the correct hostname but expired 1 hour ago.
    let initial_nonces = expired_nonces_view(&nonce, "test-host");

    let (_dir, harness) =
        setup_enroll_harness_with_declared_host("test-host", Some(&openssh), Some(initial_nonces))
            .await;

    let (csr_pem, _pubkey_der, fingerprint, _) = mint_csr_with_seed("test-host", &h_seed);
    let now = Utc::now();
    let claims = TokenClaims {
        hostname: "test-host".to_string(),
        expected_pubkey_fingerprint: fingerprint,
        issued_at: now - ChronoDuration::seconds(5),
        expires_at: now + ChronoDuration::hours(1),
        nonce,
    };
    let token = sign_token(&claims, &harness.org_root_signing_key, 1);

    let client = build_enroll_client(&harness.ca_cert);
    let req = EnrollRequest { token, csr_pem };
    let resp = client
        .post(format!("https://localhost:{}/v1/enroll", harness.port))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "expired allowlist entry must reject (nonce_allowlist_expired)",
    );

    harness.handle.abort();
}

/// Host hasn't been declared in fleet.nix at all. Closes #9: enrollment
/// is gated on the operator having added the host (with pubkey)
/// declaratively first - there's no permissive fallback.
#[tokio::test]
async fn enroll_rejects_when_host_not_declared_in_fleet() {
    use rand::RngCore;
    let mut h_seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut h_seed);
    // Fleet declares a DIFFERENT host; "test-host" is absent.
    let other_openssh = openssh_pubkey_from_seed(&h_seed);
    // No allowlist needed: nonce_not_allowlisted fires first and returns 401.
    let (_dir, harness) =
        setup_enroll_harness_with_declared_host("some-other-host", Some(&other_openssh), None)
            .await;

    let (csr_pem, _, fingerprint, _) = mint_csr_with_seed("test-host", &h_seed);
    let now = Utc::now();
    let claims = TokenClaims {
        hostname: "test-host".to_string(),
        expected_pubkey_fingerprint: fingerprint,
        issued_at: now - ChronoDuration::seconds(5),
        expires_at: now + ChronoDuration::hours(1),
        nonce: random_nonce(),
    };
    let token = sign_token(&claims, &harness.org_root_signing_key, 1);

    let client = build_enroll_client(&harness.ca_cert);
    let req = EnrollRequest { token, csr_pem };
    let resp = client
        .post(format!("https://localhost:{}/v1/enroll", harness.port))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "undeclared host must reject (declarative-enrollment policy)",
    );

    harness.handle.abort();
}
