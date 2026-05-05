//! GitOps closure: stub HTTP serves signed fleet.resolved bytes; poll task picks them up.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use common::build_fleet_resolved_json;
use ed25519_dalek::{Signer, SigningKey};
use nixfleet_control_plane::polling::channel_refs_poll::{
    spawn, ChannelRefsCache, ChannelRefsSource,
};
use nixfleet_proto::FleetResolved;
use rand::rngs::OsRng;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

async fn spawn_stub_http(
    artifact_path: &'static str,
    artifact_body: Vec<u8>,
    signature_path: &'static str,
    signature_body: Vec<u8>,
) -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let handle = tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(p) => p,
                Err(_) => return,
            };
            let artifact_clone = artifact_body.clone();
            let signature_clone = signature_body.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = match socket.read(&mut buf).await {
                    Ok(n) => n,
                    Err(_) => return,
                };
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                // FOOTGUN: match sig path first; artifact path is a prefix of it.
                let target_body = if req.contains(signature_path) {
                    Some(signature_clone)
                } else if req.contains(artifact_path) {
                    Some(artifact_clone)
                } else {
                    None
                };

                let resp: Vec<u8> = match target_body {
                    Some(body) => {
                        // FOOTGUN: stub handles one request per accept; keepalive deadlocks the second GET.
                        let mut header = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                            body.len(),
                        )
                        .into_bytes();
                        header.extend_from_slice(&body);
                        header
                    }
                    None => {
                        "HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
                            .as_bytes()
                            .to_vec()
                    }
                };
                let _ = socket.write_all(&resp).await;
                let _ = socket.flush().await;
            });
        }
    });

    (port, handle)
}

fn init_tracing() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new(
                        "warn,nixfleet_control_plane::polling::channel_refs_poll=debug",
                    )
                }),
            )
            .try_init();
    });
}

#[tokio::test]
async fn poll_refreshes_verified_fleet_snapshot() {
    init_tracing();
    let dir = TempDir::new().unwrap();

    let signing_key = SigningKey::generate(&mut OsRng);
    let public_b64 = base64::engine::general_purpose::STANDARD.encode(signing_key.verifying_key());

    let (raw_json, canonical_bytes) =
        build_fleet_resolved_json("decl0001-nixos-system-test-host-26.05", "deadbeef00000000");
    let signature = signing_key.sign(&canonical_bytes);

    let trust_path = dir.path().join("trust.json");
    let trust = serde_json::json!({
        "schemaVersion": 1,
        "ciReleaseKey": {
            "current": { "algorithm": "ed25519", "public": public_b64 },
            "previous": null,
            "rejectBefore": null,
        },
        "cacheKeys": [],
        "orgRootKey": null,
    });
    std::fs::write(&trust_path, trust.to_string()).unwrap();

    let token_path = dir.path().join("token");
    std::fs::write(&token_path, "fake-token").unwrap();

    let artifact_route = "/raw/branch/main/releases/fleet.resolved.json";
    let signature_route = "/raw/branch/main/releases/fleet.resolved.json.sig";
    let (port, _stub) = spawn_stub_http(
        artifact_route,
        raw_json.into_bytes(),
        signature_route,
        signature.to_bytes().to_vec(),
    )
    .await;

    let cache = Arc::new(RwLock::new(ChannelRefsCache::default()));
    let verified_fleet: Arc<RwLock<Option<nixfleet_control_plane::server::VerifiedFleetSnapshot>>> =
        Arc::new(RwLock::new(None));

    let cfg = ChannelRefsSource {
        artifact_url: format!("http://127.0.0.1:{port}{artifact_route}"),
        signature_url: format!("http://127.0.0.1:{port}{signature_route}"),
        token_file: Some(token_path),
        trust_path,
        freshness_window: Duration::from_secs(86400 * 365 * 5),
    };

    let last_deferrals = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
    let artifact_primed = Arc::new(AtomicBool::new(false));
    let _poll = spawn(
        CancellationToken::new(),
        cache.clone(),
        verified_fleet.clone(),
        None,
        last_deferrals,
        cfg,
        None, // no event kick in tests
        artifact_primed.clone(),
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut last_snapshot: Option<Arc<FleetResolved>> = None;
    while std::time::Instant::now() < deadline {
        if let Some(snap) = verified_fleet.read().await.clone() {
            last_snapshot = Some(snap.fleet);
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let fleet =
        last_snapshot.expect("verified_fleet snapshot should have been refreshed by the poll");
    assert_eq!(
        fleet
            .hosts
            .get("test-host")
            .and_then(|h| h.closure_hash.as_deref()),
        Some("decl0001-nixos-system-test-host-26.05"),
        "snapshot should carry the fetched closureHash",
    );
    assert_eq!(fleet.meta.ci_commit.as_deref(), Some("deadbeef00000000"));

    let refs = cache.read().await.refs.clone();
    assert!(
        refs.contains_key("stable"),
        "channel_refs should include stable: {refs:?}"
    );

    // #95: the first successful poll must flip the readiness flag so /v1/*
    // opens up - without this the daemon would serve 503 forever even
    // though the fleet snapshot is verified and live.
    assert!(
        artifact_primed.load(Ordering::Acquire),
        "first successful poll must flip artifact_primed (readiness gate)",
    );
}

#[tokio::test]
async fn poll_retains_snapshot_on_verify_failure() {
    let dir = TempDir::new().unwrap();

    let signing_key = SigningKey::generate(&mut OsRng);
    let wrong_key = SigningKey::generate(&mut OsRng);
    let wrong_public_b64 =
        base64::engine::general_purpose::STANDARD.encode(wrong_key.verifying_key());

    let (raw_json, canonical_bytes) =
        build_fleet_resolved_json("decl0001-nixos-system-test-host-26.05", "cafebabe00000000");
    let signature = signing_key.sign(&canonical_bytes);

    let trust_path = dir.path().join("trust.json");
    let trust = serde_json::json!({
        "schemaVersion": 1,
        "ciReleaseKey": {
            "current": { "algorithm": "ed25519", "public": wrong_public_b64 },
            "previous": null,
            "rejectBefore": null,
        },
        "cacheKeys": [],
        "orgRootKey": null,
    });
    std::fs::write(&trust_path, trust.to_string()).unwrap();

    let token_path = dir.path().join("token");
    std::fs::write(&token_path, "fake-token").unwrap();

    let artifact_route = "/raw/branch/main/releases/fleet.resolved.json";
    let signature_route = "/raw/branch/main/releases/fleet.resolved.json.sig";
    let (port, _stub) = spawn_stub_http(
        artifact_route,
        raw_json.into_bytes(),
        signature_route,
        signature.to_bytes().to_vec(),
    )
    .await;

    let sentinel: FleetResolved = serde_json::from_str(&serde_json::json!({
        "schemaVersion": 1,
        "hosts": { "sentinel": { "system": "x86_64-linux", "tags": [], "channel": "stable", "closureHash": "sentinel-hash", "pubkey": null } },
        "channels": { "stable": { "rolloutPolicy": "x", "reconcileIntervalMinutes": 1, "freshnessWindow": 1, "signingIntervalMinutes": 1, "compliance": { "mode": "disabled", "frameworks": [] } } },
        "rolloutPolicies": {"default":{"strategy":"waves","waves":[],"healthGate":{},"onHealthFailure":"halt"}},
        "waves": {},
        "edges": [],
        "disruptionBudgets": [],
        "meta": { "schemaVersion": 1, "signedAt": "2025-01-01T00:00:00Z", "ciCommit": "old-rev", "signatureAlgorithm": "ed25519" },
    }).to_string()).unwrap();

    let cache = Arc::new(RwLock::new(ChannelRefsCache::default()));
    let verified_fleet: Arc<RwLock<Option<nixfleet_control_plane::server::VerifiedFleetSnapshot>>> =
        Arc::new(RwLock::new(Some(
            nixfleet_control_plane::server::VerifiedFleetSnapshot {
                fleet: Arc::new(sentinel),
                fleet_resolved_hash: "sentinel-hash".to_string(),
            },
        )));

    let cfg = ChannelRefsSource {
        artifact_url: format!("http://127.0.0.1:{port}{artifact_route}"),
        signature_url: format!("http://127.0.0.1:{port}{signature_route}"),
        token_file: Some(token_path),
        trust_path,
        freshness_window: Duration::from_secs(86400 * 365 * 5),
    };

    let last_deferrals = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
    let artifact_primed = Arc::new(AtomicBool::new(false));
    let _poll = spawn(
        CancellationToken::new(),
        cache.clone(),
        verified_fleet.clone(),
        None,
        last_deferrals,
        cfg,
        None, // no event kick in tests
        artifact_primed.clone(),
    );

    // GOTCHA: negative-observation test - fixed sleep is correct because no positive condition can converge.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let snapshot = verified_fleet.read().await.clone();
    let fleet = snapshot
        .expect("sentinel must be retained on verify failure")
        .fleet;
    assert_eq!(
        fleet
            .hosts
            .get("sentinel")
            .and_then(|h| h.closure_hash.as_deref()),
        Some("sentinel-hash"),
        "verify-failure must NOT overwrite sentinel snapshot",
    );

    // #95: a verify-failed poll must NOT flip the readiness flag - even
    // with a sentinel snapshot already in place from a prior boot.
    // Otherwise the rebuild-resurrects-revoked-cert path opens up.
    assert!(
        !artifact_primed.load(Ordering::Acquire),
        "verify failure must not flip artifact_primed",
    );
}
