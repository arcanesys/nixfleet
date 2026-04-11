//! Phase 4 — mTLS CN validation middleware (defense in depth).
//!
//! The middleware lives in `control-plane/src/auth_cn.rs` and is wired
//! on agent-facing routes only via `lib.rs::build_app`. These tests
//! exercise the middleware directly by injecting a synthesized
//! `PeerCertificates` extension into the request, since the test
//! harness uses raw TCP (no TLS) and therefore no real peer cert.
//!
//! Coverage:
//!   1. No PeerCertificates extension at all → no-op (harness path).
//!   2. PeerCertificates present but empty → no-op (mTLS not configured).
//!   3. PeerCertificates with CN matching the path id → 200.
//!   4. PeerCertificates with CN NOT matching → 403.
//!
//! Cases 3 and 4 require a real DER-encoded cert with a known CN. We
//! generate one in-test using a simple self-signed cert built via
//! `rcgen`, which is already a transitive dep of the rustls stack but
//! NOT a direct dep — we add it as a dev-dep in this PR.
//!
//! For the no-op cases (1, 2) we don't need rcgen because we either
//! omit the extension entirely or insert the empty `PeerCertificates`
//! constructor.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nixfleet_control_plane::auth_cn::PeerCertificates;
use rustls_pki_types::CertificateDer;
use tower::util::ServiceExt;

#[path = "harness.rs"]
mod harness;

/// Build a tiny axum router that mounts the same agent-route layer as
/// `build_app`, plus a 200-OK handler at the same path. We do not use
/// `build_app` here because the harness's spawn_cp() variant constructs
/// the full app over a TCP listener and that path doesn't carry
/// PeerCertificates extensions. For these tests we want to exercise
/// the middleware directly via tower's `oneshot` against an in-process
/// router.
fn agent_router_with_cn_layer() -> axum::Router {
    use axum::routing::get;
    axum::Router::new()
        .route(
            "/api/v1/machines/{id}/desired-generation",
            get(|axum::extract::Path(id): axum::extract::Path<String>| async move {
                format!("ok-{id}")
            }),
        )
        .layer(axum::middleware::from_fn(
            nixfleet_control_plane::auth_cn::cn_matches_path_machine_id,
        ))
}

/// Case 1: no PeerCertificates extension at all (raw HTTP harness
/// path). The middleware lets the request through.
#[tokio::test]
async fn no_peer_cert_extension_lets_request_through() {
    let app = agent_router_with_cn_layer();
    let req = Request::builder()
        .uri("/api/v1/machines/web-01/desired-generation")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Case 2: PeerCertificates extension is present but empty (mTLS not
/// configured at the TLS layer — the MtlsAcceptor still inserts an
/// empty extension on every connection so the middleware sees a
/// uniform shape). Middleware lets the request through.
#[tokio::test]
async fn empty_peer_cert_extension_lets_request_through() {
    let app = agent_router_with_cn_layer();
    let mut req = Request::builder()
        .uri("/api/v1/machines/web-01/desired-generation")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(PeerCertificates::empty());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Build a self-signed cert with the given CN and return the leaf
/// DER bytes wrapped in a PeerCertificates suitable for the test
/// extension.
fn peer_certs_with_cn(cn: &str) -> PeerCertificates {
    let mut params = rcgen::CertificateParams::new(vec![cn.to_string()]).unwrap();
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, cn);
    params.distinguished_name = dn;
    let key_pair = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key_pair).unwrap();
    let der = cert.der().to_vec();
    PeerCertificates::new(vec![CertificateDer::from(der)])
}

/// Case 3: PeerCertificates with CN matching the path id → 200.
#[tokio::test]
async fn matching_cn_passes_through() {
    let app = agent_router_with_cn_layer();
    let mut req = Request::builder()
        .uri("/api/v1/machines/web-01/desired-generation")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(peer_certs_with_cn("web-01"));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Case 4: PeerCertificates with CN NOT matching the path id → 403.
/// This is the impersonation defense.
#[tokio::test]
async fn mismatched_cn_returns_403() {
    let app = agent_router_with_cn_layer();
    let mut req = Request::builder()
        .uri("/api/v1/machines/web-02/desired-generation")
        .body(Body::empty())
        .unwrap();
    // Cert is for web-01 but the path says web-02 — impersonation.
    req.extensions_mut().insert(peer_certs_with_cn("web-01"));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// harness reference is intentional — keeps cargo from declaring it
// unused at the file level. Real harness usage in this file is nil
// because every case here uses the in-process axum::Router pattern.
#[allow(dead_code)]
fn _harness_marker() -> harness::Cp {
    unreachable!("compile-only marker")
}
