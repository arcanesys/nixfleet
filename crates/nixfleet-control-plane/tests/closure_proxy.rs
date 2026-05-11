//! Integration tests for `/v1/agent/closure/{hash}` proxy.

mod common;

use std::net::SocketAddr;
use std::path::PathBuf;

use common::{
    build_mtls_client, install_crypto_provider_once, mint_ca_and_certs, pick_free_port,
    wait_for_listener_ready, write_phase2_input_stubs,
};
use nixfleet_control_plane::server;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn spawn_cp(
    dir: &TempDir,
    server_cert: PathBuf,
    server_key: PathBuf,
    ca: PathBuf,
    cp_port: u16,
    closure_upstream: Option<String>,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    let (artifact, signature, trust, observed) = write_phase2_input_stubs(dir);
    let args = server::ServeArgs {
        listen: format!("127.0.0.1:{cp_port}").parse().unwrap(),
        tls_cert: server_cert,
        tls_key: server_key,
        client_ca: Some(ca),
        artifact_path: artifact,
        signature_path: signature,
        trust_path: trust,
        observed_path: observed,
        confirm_deadline_secs: 120,
        closure_upstream,
        mark_ready_at_startup: true,
        ..Default::default()
    };
    let handle = tokio::spawn(server::serve(args));
    wait_for_listener_ready(cp_port, &handle).await;
    handle
}

async fn stub_http_once(addr: SocketAddr, body: &'static str) -> tokio::task::JoinHandle<()> {
    let listener = TcpListener::bind(addr).await.unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        let n = socket.read(&mut buf).await.unwrap();
        let _ = &buf[..n];
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/x-nix-narinfo\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(resp.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
    })
}

#[tokio::test]
async fn closure_proxy_returns_501_when_upstream_unset() {
    install_crypto_provider_once();
    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "test-host");

    let cp_port = pick_free_port().await;
    let handle = spawn_cp(&dir, server_cert, server_key, ca.clone(), cp_port, None).await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);
    let resp = client
        .get(format!(
            "https://localhost:{cp_port}/v1/agent/closure/abc123"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 501);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("closure proxy not configured"),
        "body: {body}"
    );

    handle.abort();
}

#[tokio::test]
async fn closure_proxy_forwards_to_upstream() {
    install_crypto_provider_once();
    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "test-host");

    // GOTCHA: stub must bind before CP spawn so URL resolves at startup.
    let upstream_port = pick_free_port().await;
    let upstream_addr: SocketAddr = format!("127.0.0.1:{upstream_port}").parse().unwrap();
    let stub_body = "StorePath: /nix/store/abc123-test\nURL: nar/abc.nar.zst\n";
    let stub = stub_http_once(upstream_addr, stub_body).await;

    let cp_port = pick_free_port().await;
    let handle = spawn_cp(
        &dir,
        server_cert,
        server_key,
        ca.clone(),
        cp_port,
        Some(format!("http://127.0.0.1:{upstream_port}")),
    )
    .await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);
    let resp = client
        .get(format!(
            "https://localhost:{cp_port}/v1/agent/closure/abc123"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert_eq!(body, stub_body);

    stub.abort();
    handle.abort();
}

#[tokio::test]
async fn closure_proxy_returns_502_when_upstream_unreachable() {
    install_crypto_provider_once();
    let dir = TempDir::new().unwrap();
    let (ca, server_cert, server_key, client_cert, client_key) =
        mint_ca_and_certs(&dir, "test-host");

    // GOTCHA: small race if another proc binds the port before the test runs; acceptable locally.
    let dead_port = pick_free_port().await;

    let cp_port = pick_free_port().await;
    let handle = spawn_cp(
        &dir,
        server_cert,
        server_key,
        ca.clone(),
        cp_port,
        Some(format!("http://127.0.0.1:{dead_port}")),
    )
    .await;

    let client = build_mtls_client(&ca, &client_cert, &client_key);
    let resp = client
        .get(format!(
            "https://localhost:{cp_port}/v1/agent/closure/abc123"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 502);

    handle.abort();
}
