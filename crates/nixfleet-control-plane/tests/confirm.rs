//! Integration tests for `/v1/agent/confirm`.

mod common;

use std::path::PathBuf;

use chrono::Utc;
use common::{
    build_mtls_client, install_crypto_provider_once, mint_ca_and_certs, pick_free_port,
    wait_for_listener_ready, write_phase2_input_stubs,
};
use nixfleet_control_plane::{
    db::{Db, DispatchInsert},
    server,
};
use nixfleet_proto::agent_wire::{ConfirmRequest, GenerationRef};
use tempfile::TempDir;

async fn spawn_server_with_db_at_port(
    args_dir: &TempDir,
    db_path: Option<PathBuf>,
    server_cert: PathBuf,
    server_key: PathBuf,
    client_ca: Option<PathBuf>,
    port: u16,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    let (artifact, signature, trust, observed) = write_phase2_input_stubs(args_dir);
    let listen: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    let args = server::ServeArgs {
        listen,
        tls_cert: server_cert,
        tls_key: server_key,
        client_ca,
        artifact_path: artifact,
        signature_path: signature,
        trust_path: trust,
        observed_path: observed,
        confirm_deadline_secs: 120,
        db_path,
        mark_ready_at_startup: true,
        ..Default::default()
    };
    let handle = tokio::spawn(server::serve(args));
    wait_for_listener_ready(port, &handle).await;
    handle
}

#[tokio::test]
async fn confirm_happy_path_marks_row_confirmed() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "test-host");
    let db_path = dir.path().join("state.db");

    {
        let db = Db::open(&db_path).unwrap();
        db.migrate().unwrap();
        let deadline = Utc::now() + chrono::Duration::seconds(120);
        db.host_dispatch_state()
            .record_dispatch(&DispatchInsert {
                hostname: "test-host",
                rollout_id: "stable@abc123",
                channel: "stable",
                wave: 0,
                target_closure_hash: "deadbeef-nixos-system",
                target_channel_ref: "main",
                confirm_deadline: deadline,
            })
            .unwrap();
    }

    let port = pick_free_port().await;
    let handle = spawn_server_with_db_at_port(
        &dir,
        Some(db_path.clone()),
        server_cert,
        server_key,
        Some(ca.clone()),
        port,
    )
    .await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);

    let req = ConfirmRequest {
        hostname: "test-host".to_string(),
        rollout: "stable@abc123".to_string(),
        wave: 0,
        generation: GenerationRef {
            closure_hash: "deadbeef-nixos-system".to_string(),
            channel_ref: Some("main".to_string()),
            boot_id: "00000000-0000-0000-0000-000000000000".to_string(),
        },
    };

    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/confirm"))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204, "expected 204 No Content");

    let db = Db::open(&db_path).unwrap();
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let state: String = conn
        .query_row(
            "SELECT state FROM host_dispatch_state WHERE hostname=?1 AND rollout_id=?2",
            rusqlite::params!["test-host", "stable@abc123"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "confirmed");
    drop(db);

    handle.abort();
}

#[tokio::test]
async fn confirm_returns_410_when_no_pending_row() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "test-host");
    let db_path = dir.path().join("state.db");

    {
        let db = Db::open(&db_path).unwrap();
        db.migrate().unwrap();
    }

    let port = pick_free_port().await;
    let handle = spawn_server_with_db_at_port(
        &dir,
        Some(db_path.clone()),
        server_cert,
        server_key,
        Some(ca.clone()),
        port,
    )
    .await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);

    let req = ConfirmRequest {
        hostname: "test-host".to_string(),
        rollout: "rollout-that-doesnt-exist".to_string(),
        wave: 0,
        generation: GenerationRef {
            closure_hash: "abc".to_string(),
            channel_ref: None,
            boot_id: "boot".to_string(),
        },
    };

    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/confirm"))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 410, "expected 410 Gone (no matching row)");

    handle.abort();
}

#[tokio::test]
async fn confirm_rejects_cn_hostname_mismatch() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "test-host");
    let db_path = dir.path().join("state.db");
    {
        let db = Db::open(&db_path).unwrap();
        db.migrate().unwrap();
    }

    let port = pick_free_port().await;
    let handle = spawn_server_with_db_at_port(
        &dir,
        Some(db_path),
        server_cert,
        server_key,
        Some(ca.clone()),
        port,
    )
    .await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);

    let req = ConfirmRequest {
        hostname: "ohm".to_string(),
        rollout: "any".to_string(),
        wave: 0,
        generation: GenerationRef {
            closure_hash: "abc".to_string(),
            channel_ref: None,
            boot_id: "boot".to_string(),
        },
    };

    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/confirm"))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "expected 403 on CN/hostname mismatch");

    handle.abort();
}

#[tokio::test]
async fn confirm_returns_503_without_db() {
    install_crypto_provider_once();

    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "test-host");

    let port = pick_free_port().await;
    let handle = spawn_server_with_db_at_port(
        &dir,
        None,
        server_cert,
        server_key,
        Some(ca.clone()),
        port,
    )
    .await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);

    let req = ConfirmRequest {
        hostname: "test-host".to_string(),
        rollout: "any".to_string(),
        wave: 0,
        generation: GenerationRef {
            closure_hash: "abc".to_string(),
            channel_ref: None,
            boot_id: "boot".to_string(),
        },
    };

    let resp = client
        .post(format!("https://localhost:{port}/v1/agent/confirm"))
        .json(&req)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        503,
        "expected 503 Service Unavailable when no DB"
    );

    handle.abort();
}
