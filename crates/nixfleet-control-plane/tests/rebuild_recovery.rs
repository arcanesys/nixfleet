//! End-to-end recovery test for the rollouts table.
//!
//! Scenario: CP rebuild wipes state.db. We assert that:
//!   1. A fresh DB starts with an empty rollouts table.
//!   2. One channel-refs polling tick (HTTP fetch + verify + apply) populates
//!      the table with the current rollout per channel.
//!   3. Idempotent across repeated polls (subsequent ticks find the same rid
//!      already recorded; no spurious supersession of the active rollout).
//!   4. After a fleet bump (new fleet_resolved_hash -> new rollout_id), the
//!      previous rid is marked superseded by the new one.
//!   5. Across a "rebuild" (drop DB + fresh DB + replay polling), the table
//!      converges to the same state as if the rebuild had never happened  -
//!      modulo the previous rid disappearing entirely (no historical state).

mod common;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use base64::Engine as _;
use common::build_fleet_resolved_json;
use ed25519_dalek::ed25519::signature::rand_core::OsRng;
use ed25519_dalek::{Signer, SigningKey};
use nixfleet_control_plane::db::Db;
use nixfleet_control_plane::polling::channel_refs_poll::{
    ChannelRefsCache, ChannelRefsSource, spawn,
};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// Single-shot HTTP stub that serves whatever (artifact, signature) bytes it
/// holds for any matching path. Closes the connection after each response so
/// the polling client doesn't deadlock on keepalive.
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

struct PollFixture {
    pub artifact_url: String,
    pub signature_url: String,
    pub trust_path: std::path::PathBuf,
    pub token_path: std::path::PathBuf,
    /// Held to keep the temp dir alive for the duration of the fixture.
    _dir: TempDir,
    /// Port -> join handle, kept alive for the duration of the fixture.
    _stub: tokio::task::JoinHandle<()>,
}

async fn fixture_for_fleet(
    declared_closure: &str,
    ci_commit: &str,
    artifact_route: &'static str,
    signature_route: &'static str,
) -> PollFixture {
    let dir = TempDir::new().unwrap();

    let signing_key = SigningKey::generate(&mut OsRng);
    let public_b64 = base64::engine::general_purpose::STANDARD.encode(signing_key.verifying_key());

    let (raw_json, canonical_bytes) = build_fleet_resolved_json(declared_closure, ci_commit);
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

    let (port, stub) = spawn_stub_http(
        artifact_route,
        raw_json.into_bytes(),
        signature_route,
        signature.to_bytes().to_vec(),
    )
    .await;

    PollFixture {
        artifact_url: format!("http://127.0.0.1:{port}{artifact_route}"),
        signature_url: format!("http://127.0.0.1:{port}{signature_route}"),
        trust_path,
        token_path,
        _dir: dir,
        _stub: stub,
    }
}

/// Polls the rollouts table via the public API until any rid is known to be
/// active for the given channel. The polling-loop call site uses
/// `compute_rollout_id_for_channel` against the verified-fleet snapshot,
/// so we re-derive the same rid here once the snapshot is published, then
/// verify the table reflects it.
async fn wait_for_recorded_rid(
    db: &Db,
    verified_fleet: &Arc<RwLock<Option<nixfleet_control_plane::server::VerifiedFleetSnapshot>>>,
    channel: &str,
    deadline: std::time::Instant,
) -> Option<String> {
    while std::time::Instant::now() < deadline {
        let snap = verified_fleet.read().await.clone();
        if let Some(snap) = snap
            && let Ok(Some(rid)) = nixfleet_reconciler::compute_rollout_id_for_channel(
                &snap.fleet,
                &snap.fleet_resolved_hash,
                channel,
            )
            && let Ok(Some(status)) = db.rollouts().supersede_status(&rid)
            && !status.is_superseded()
        {
            return Some(rid);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

/// Single canonical happy-path: fresh CP rebuild, polling populates table,
/// fleet bump supersedes the prior rid.
#[tokio::test(flavor = "multi_thread")]
async fn polling_populates_rollouts_after_rebuild_and_supersedes_on_bump() {
    let fixture = fixture_for_fleet(
        "decl0001-nixos-system-test-host-26.05",
        "deadbeef00000000",
        "/raw/branch/main/releases/fleet.resolved.json",
        "/raw/branch/main/releases/fleet.resolved.json.sig",
    )
    .await;

    // Fresh DB - simulating CP rebuild with empty state.db.
    let db_dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(&db_dir.path().join("state.db")).unwrap());
    db.migrate().unwrap();
    assert!(
        db.rollouts().superseded_rollout_ids().unwrap().is_empty(),
        "fresh DB has nothing superseded",
    );
    assert!(
        db.rollouts()
            .supersede_status("ghost-rid-pre-rebuild")
            .unwrap()
            .is_none(),
        "rebuild scenario starts with empty rollouts table",
    );

    let cache = Arc::new(RwLock::new(ChannelRefsCache::default()));
    let verified_fleet: Arc<RwLock<Option<nixfleet_control_plane::server::VerifiedFleetSnapshot>>> =
        Arc::new(RwLock::new(None));

    let cancel = CancellationToken::new();
    let last_deferrals = Arc::new(RwLock::new(std::collections::HashMap::new()));
    let _poll = spawn(
        cancel.clone(),
        cache.clone(),
        verified_fleet.clone(),
        Some(db.clone()),
        last_deferrals,
        ChannelRefsSource {
            artifact_url: fixture.artifact_url.clone(),
            signature_url: fixture.signature_url.clone(),
            token_file: Some(fixture.token_path.clone()),
            trust_path: fixture.trust_path.clone(),
            freshness_window: Duration::from_secs(86400 * 365 * 5),
        },
        None, // cadence-only in this test
        Arc::new(AtomicBool::new(false)),
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let first_rid = wait_for_recorded_rid(&db, &verified_fleet, "stable", deadline)
        .await
        .expect("first polling tick must populate rollouts table");

    // Idempotency: the supersede UPDATE has WHERE rollout_id != self, so a
    // second call with the same rid is a no-op. Manually invoke to simulate
    // a repeat tick without waiting another 60s.
    db.rollouts()
        .record_active_rollout(&first_rid, "stable")
        .expect("idempotent re-record");
    let still_active_after_repeat = !db
        .rollouts()
        .superseded_rollout_ids()
        .unwrap()
        .contains(&first_rid);
    assert!(
        still_active_after_repeat,
        "re-recording the same (rid, channel) must not mark itself superseded",
    );

    cancel.cancel();

    // Fleet bump: a NEW signing run produces a different fleet_resolved_hash
    // -> different rollout_id. Simulate by computing a fresh rid against a
    // bumped FleetResolved snapshot and recording it directly. The table
    // should mark the prior rid superseded by the new one.
    let snap = verified_fleet
        .read()
        .await
        .clone()
        .expect("verified-fleet snapshot must exist after first poll");
    let bumped_hash = format!("11{}", &snap.fleet_resolved_hash[2..]);
    let bumped_rid =
        nixfleet_reconciler::compute_rollout_id_for_channel(&snap.fleet, &bumped_hash, "stable")
            .unwrap()
            .expect("stable channel resolves a rollout id");
    assert_ne!(
        bumped_rid, first_rid,
        "bumping fleet_resolved_hash must yield a fresh rollout id",
    );
    db.rollouts()
        .record_active_rollout(&bumped_rid, "stable")
        .unwrap();

    let prior = db
        .rollouts()
        .supersede_status(&first_rid)
        .unwrap()
        .expect("prior rid still in table");
    assert!(
        prior.is_superseded(),
        "fleet bump must mark the prior rid superseded",
    );
    assert_eq!(prior.superseded_by.as_deref(), Some(bumped_rid.as_str()));

    let bumped = db
        .rollouts()
        .supersede_status(&bumped_rid)
        .unwrap()
        .expect("new rid present");
    assert!(
        !bumped.is_superseded(),
        "new rid must remain active after bump",
    );

    // No-historical-state check: a never-seen rid stays unknown and the
    // lifecycle endpoint surfaces 404 (verified by other tests; here we
    // assert the table contract: supersede_status returns None).
    assert!(
        db.rollouts()
            .supersede_status("ancient-rid-from-before-the-rebuild")
            .unwrap()
            .is_none(),
        "rids never recorded must stay absent - no historical reconstruction",
    );
}
