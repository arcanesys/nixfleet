//! End-to-end: spin a real CP, drive 2 agent checkins via mTLS, then call
//! `run_status` as an operator. No wire-layer mocking - real signing, real
//! mTLS chain, real `/v1/agent/checkin` + `/v1/hosts` traffic.

use std::path::PathBuf;
use std::sync::Once;
use std::time::{Duration, Instant};

use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use nixfleet_cli::{run_status, ResolvedClientConfig};
use nixfleet_control_plane::server;
use nixfleet_proto::agent_wire::{
    CheckinRequest, CheckinResponse, FetchOutcome, FetchResult, GenerationRef,
};
use rand::rngs::OsRng;
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use reqwest::{Certificate as ReqwestCert, Identity};
use tempfile::TempDir;
use tokio::net::{TcpListener, TcpStream};

const HOST_A: &str = "host-a";
const HOST_B: &str = "host-b";
const CHANNEL: &str = "stable";
const DECLARED_CLOSURE: &str = "decl-e2e-deadbeef";
const CI_COMMIT: &str = "abcdef0011223344";

static CRYPTO: Once = Once::new();
fn install_crypto() {
    CRYPTO.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    });
}

async fn pick_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn wait_ready(port: u16, handle: &tokio::task::JoinHandle<anyhow::Result<()>>) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if handle.is_finished() {
            panic!("server task exited before listener bound (likely TLS or fleet-artifact error)");
        }
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("listener did not bind in 15s");
}

fn write_pem(dir: &TempDir, name: &str, body: &str) -> PathBuf {
    let p = dir.path().join(name);
    std::fs::write(&p, body).unwrap();
    p
}

fn write_bytes(dir: &TempDir, name: &str, body: &[u8]) -> PathBuf {
    let p = dir.path().join(name);
    std::fs::write(&p, body).unwrap();
    p
}

struct PkiBundle {
    ca_pem_path: PathBuf,
    server_cert: PathBuf,
    server_key: PathBuf,
    /// `(cert_pem_path, key_pem_path)` - one entry per CN passed to mint, in order.
    clients: Vec<(PathBuf, PathBuf)>,
}

/// Mint a self-signed CA + server cert (SAN=localhost) + one client cert per
/// CN. All written to PEM under `dir`.
fn mint_pki(dir: &TempDir, client_cns: &[&str]) -> PkiBundle {
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
    let ca_pem_path = write_pem(dir, "ca.pem", &ca_cert.pem());

    let mut server_params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    server_params
        .distinguished_name
        .push(DnType::CommonName, "test-cp-server");
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_key = KeyPair::generate().unwrap();
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .unwrap();
    let server_cert_path = write_pem(dir, "server.pem", &server_cert.pem());
    let server_key_path = write_pem(dir, "server.key", &server_key.serialize_pem());

    let mut clients = Vec::new();
    for (i, cn) in client_cns.iter().enumerate() {
        let mut p = CertificateParams::default();
        p.distinguished_name.push(DnType::CommonName, *cn);
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let k = KeyPair::generate().unwrap();
        let c = p.signed_by(&k, &ca_cert, &ca_key).unwrap();
        let cert_path = write_pem(dir, &format!("client{i}.pem"), &c.pem());
        let key_path = write_pem(dir, &format!("client{i}.key"), &k.serialize_pem());
        clients.push((cert_path, key_path));
    }

    PkiBundle {
        ca_pem_path,
        server_cert: server_cert_path,
        server_key: server_key_path,
        clients,
    }
}

fn build_fleet_resolved(declared: &str) -> (String, Vec<u8>) {
    let json = serde_json::json!({
        "schemaVersion": 1,
        "hosts": {
            HOST_A: {
                "system": "x86_64-linux", "tags": [],
                "channel": CHANNEL, "closureHash": declared, "pubkey": null,
            },
            HOST_B: {
                "system": "x86_64-linux", "tags": [],
                "channel": CHANNEL, "closureHash": declared, "pubkey": null,
            },
        },
        "channels": {
            CHANNEL: {
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
            "signedAt": "2026-05-05T00:00:00Z",
            "ciCommit": CI_COMMIT,
            "signatureAlgorithm": "ed25519",
        },
    });
    let raw = serde_json::to_string(&json).unwrap();
    let canonical = nixfleet_canonicalize::canonicalize(&raw).unwrap();
    (raw, canonical.into_bytes())
}

/// Mint a release key, sign the canonical fleet bytes, write
/// `(artifact, signature, trust)` using the canonical `TrustConfig` shape
/// (the looser list shape is not accepted by the prod parser).
fn write_signed_fleet(dir: &TempDir) -> (PathBuf, PathBuf, PathBuf) {
    let signing = SigningKey::generate(&mut OsRng);
    let pub_b64 = base64::engine::general_purpose::STANDARD.encode(signing.verifying_key());
    let (raw, canonical) = build_fleet_resolved(DECLARED_CLOSURE);
    let sig = signing.sign(&canonical);
    let artifact = write_pem(dir, "fleet.resolved.json", &raw);
    let signature = write_bytes(dir, "fleet.resolved.json.sig", &sig.to_bytes());
    let trust_json = serde_json::json!({
        "schemaVersion": 1,
        "ciReleaseKey": {
            "current": { "algorithm": "ed25519", "public": pub_b64 },
            "previous": null,
            "rejectBefore": null,
        },
        "cacheKeys": [],
        "orgRootKey": null,
    });
    let trust = write_pem(dir, "trust.json", &trust_json.to_string());
    (artifact, signature, trust)
}

fn build_mtls(ca: &PathBuf, cert: &PathBuf, key: &PathBuf) -> reqwest::Client {
    let mut pem = std::fs::read(cert).unwrap();
    pem.extend_from_slice(&std::fs::read(key).unwrap());
    let identity = Identity::from_pem(&pem).unwrap();
    let ca_pem = std::fs::read(ca).unwrap();
    reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(ReqwestCert::from_pem(&ca_pem).unwrap())
        .identity(identity)
        .build()
        .unwrap()
}

async fn checkin(
    cp_url: &str,
    client: &reqwest::Client,
    hostname: &str,
    current_closure: &str,
) -> CheckinResponse {
    let req = CheckinRequest {
        hostname: hostname.into(),
        agent_version: "0.2.0".into(),
        current_generation: GenerationRef {
            closure_hash: current_closure.into(),
            channel_ref: None,
            boot_id: "00000000-0000-0000-0000-000000000000".into(),
        },
        pending_generation: None,
        last_evaluated_target: None,
        last_fetch_outcome: Some(FetchOutcome {
            result: FetchResult::Ok,
            error: None,
        }),
        uptime_secs: Some(42),
        last_confirmed_at: None,
        attestation_signature: None,
        health_probes: vec![],
        health_check_mode: None,
    };
    client
        .post(format!("{cp_url}/v1/agent/checkin"))
        .json(&req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn nixfleet_status_renders_two_real_hosts_after_checkins() {
    install_crypto();

    let dir = TempDir::new().unwrap();
    let pki = mint_pki(&dir, &[HOST_A, HOST_B, "operator"]);
    let (artifact, signature, trust) = write_signed_fleet(&dir);
    let observed = write_pem(
        &dir,
        "observed.json",
        r#"{"channelRefs":{},"lastRolledRefs":{},"hostState":{},"activeRollouts":[]}"#,
    );

    let port = pick_port().await;
    let listen = format!("127.0.0.1:{port}").parse().unwrap();

    let server = tokio::spawn(server::serve(server::ServeArgs {
        listen,
        tls_cert: pki.server_cert.clone(),
        tls_key: pki.server_key.clone(),
        client_ca: Some(pki.ca_pem_path.clone()),
        artifact_path: artifact,
        signature_path: signature,
        trust_path: trust,
        observed_path: observed,
        // Multi-year window keeps the static `signedAt` fresh as wall-clock
        // advances during long test runs.
        freshness_window: Duration::from_secs(86_400 * 365 * 5),
        confirm_deadline_secs: 120,
        ..Default::default()
    }));
    wait_ready(port, &server).await;

    let cp_url = format!("https://localhost:{port}");

    // Per-host mTLS cert so the CP's CN-vs-hostname guard accepts.
    let host_a = build_mtls(&pki.ca_pem_path, &pki.clients[0].0, &pki.clients[0].1);
    let _ra = checkin(&cp_url, &host_a, HOST_A, "current-a-closure").await;
    let host_b = build_mtls(&pki.ca_pem_path, &pki.clients[1].0, &pki.clients[1].1);
    let _rb = checkin(&cp_url, &host_b, HOST_B, "current-b-closure").await;

    let cfg = ResolvedClientConfig {
        cp_url: cp_url.clone(),
        ca_cert: pki.ca_pem_path.clone(),
        client_cert: pki.clients[2].0.clone(),
        client_key: pki.clients[2].1.clone(),
    };
    let rendered = run_status(&cfg, false, false).await.expect("run_status");

    // Assert load-bearing substrings; the CP timestamps row ages at call
    // time so a byte-exact snapshot would be racy.
    assert!(rendered.contains("HOST"), "header missing: {rendered}");
    assert!(rendered.contains(HOST_A), "host-a missing: {rendered}");
    assert!(rendered.contains(HOST_B), "host-b missing: {rendered}");
    assert!(rendered.contains(CHANNEL), "channel missing: {rendered}");

    // Current closure != declared, so hosts must not be converged.
    assert!(
        !rendered.contains("\u{2713} converged"),
        "freshly-divergent hosts must not be converged: {rendered}",
    );

    let json = run_status(&cfg, true, false)
        .await
        .expect("run_status json");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parseable JSON");
    assert!(parsed.get("hosts").and_then(|h| h.as_array()).is_some());

    server.abort();
}

#[test]
fn nixfleet_derive_pubkey_deterministic_for_fixed_key() {
    use base64::Engine as _;
    use ed25519_dalek::SigningKey;

    let dir = tempfile::TempDir::new().unwrap();
    let key_path = dir.path().join("private-key.bin");
    let seed = [0x42u8; 32];
    std::fs::write(&key_path, seed).unwrap();

    let expected = base64::engine::general_purpose::STANDARD
        .encode(SigningKey::from_bytes(&seed).verifying_key().to_bytes());

    let bin = env!("CARGO_BIN_EXE_nixfleet");
    let output = std::process::Command::new(bin)
        .args(["derive-pubkey", key_path.to_str().unwrap()])
        .output()
        .expect("spawn nixfleet derive-pubkey");

    assert!(
        output.status.success(),
        "derive-pubkey failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.trim(), expected);
}
