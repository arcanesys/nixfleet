//! mTLS defense-in-depth: extract verified peer cert + CN, expose as
//! request extension, optionally enforce CN-vs-path-id.
//!
//! Ported from v0.1's `crates/control-plane/src/auth_cn.rs` (tag
//! v0.1.1) with the upstream-attribution comments preserved. v0.1
//! shipped against the same axum-server 0.7 + tokio-rustls 0.26 +
//! x509-parser 0.16 stack, so the implementation is drop-in.
//!
//! ## Why this module exists in-tree
//!
//! `axum-server 0.7.3` does not expose peer certificates through any
//! public API ([upstream issue #162](https://github.com/programatik29/axum-server/issues/162)).
//! The standard fix is a custom `Accept` wrapper that, after the TLS
//! handshake, reads `tokio_rustls::server::TlsStream::get_ref().1.peer_certificates()`
//! and injects the chain into every request on that connection via a
//! per-connection `tower::Service` wrapper.
//!
//! The `axum-server-mtls` crate (v0.1.0) implements exactly this
//! pattern. We vendor a trimmed-down version in-tree to avoid taking
//! a 0.1.0 third-party dependency. The implementation is mechanical
//! and matches the upstream pattern.
//!
//! ## Wiring
//!
//! `server.rs` builds the `RustlsAcceptor`, wraps it in
//! `MtlsAcceptor::new(...)`, and calls
//! `axum_server::bind(addr).acceptor(mtls)`. The `MtlsAcceptor`
//! injects `PeerCertificates` into every request extension on a
//! connection. The `/v1/whoami` handler reads the extension via
//! [`PeerCertificates::leaf_cn`]. The future
//! [`cn_matches_path_machine_id`] middleware (wired by PR-3+ on
//! agent-facing routes that take a `{id}` path segment) reads the
//! extension and rejects with 403 if the CN does not match.
//!
//! When mTLS is not configured (`tls.client_ca` is None at the
//! server config level), the `PeerCertificates` extension still
//! exists but is empty (`is_present() == false`). The middleware
//! treats that as a no-op and lets the request through, so PR-1's
//! TLS-only mode (and the existing /healthz integration test) keeps
//! working.

use axum::extract::{Path, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use axum_server::accept::Accept;
use axum_server::tls_rustls::RustlsAcceptor;
use rustls_pki_types::CertificateDer;
use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::pin::Pin;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::server::TlsStream;
use x509_parser::prelude::*;

// =====================================================================
// PeerCertificates — minimal cert chain wrapper, leaf_cn only
// =====================================================================

/// Client certificate chain extracted from the TLS connection,
/// injected into every request as an extension by [`MtlsAcceptor`].
/// If the client did not present a certificate the chain is empty.
#[derive(Clone, Debug, Default)]
pub struct PeerCertificates {
    /// DER-encoded certificate chain, leaf first. Index 0 is the
    /// client's own certificate.
    chain: Vec<CertificateDer<'static>>,
}

impl PeerCertificates {
    pub fn new(chain: Vec<CertificateDer<'static>>) -> Self {
        Self { chain }
    }

    pub fn empty() -> Self {
        Self { chain: Vec::new() }
    }

    pub fn is_present(&self) -> bool {
        !self.chain.is_empty()
    }

    pub fn leaf(&self) -> Option<&CertificateDer<'static>> {
        self.chain.first()
    }

    /// Extract the Common Name from the leaf certificate's subject.
    /// Returns `None` if no certificate is present or the CN cannot
    /// be parsed.
    pub fn leaf_cn(&self) -> Option<String> {
        let leaf = self.leaf()?;
        let (_, cert) = X509Certificate::from_der(leaf.as_ref()).ok()?;
        // Bind the result to a local before the block ends so the
        // x509-parser temporary holding the borrow is dropped first.
        let cn = cert
            .subject()
            .iter_common_name()
            .next()
            .and_then(|attr| attr.as_str().ok().map(String::from));
        cn
    }
}

// =====================================================================
// MtlsAcceptor — wraps RustlsAcceptor, injects peer certs
// =====================================================================

/// Wraps a [`RustlsAcceptor`] so that the peer certificate chain is
/// extracted after the TLS handshake and injected into every request
/// on that connection via a per-connection [`PeerCertService`]
/// wrapper.
///
/// Built from an existing `RustlsAcceptor` so the operator's TLS
/// config (cert, key, optional client CA) flows through unchanged.
#[derive(Clone, Debug)]
pub struct MtlsAcceptor<A = axum_server::accept::DefaultAcceptor> {
    inner: RustlsAcceptor<A>,
}

impl MtlsAcceptor {
    pub fn new(inner: RustlsAcceptor) -> Self {
        Self { inner }
    }
}

impl<I, S, A> Accept<I, S> for MtlsAcceptor<A>
where
    A: Accept<I, S> + Clone + Send + 'static,
    A::Stream: AsyncRead + AsyncWrite + Unpin + Send,
    A::Service: Send,
    A::Future: Send,
    I: Send + 'static,
    S: Send + 'static,
{
    type Stream = TlsStream<A::Stream>;
    type Service = PeerCertService<A::Service>;
    type Future = Pin<Box<dyn Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send>>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let inner = self.inner.clone();
        Box::pin(async move {
            // Delegate the TLS handshake to RustlsAcceptor.
            let (tls_stream, inner_service) = inner.accept(stream, service).await?;

            // After the handshake, get_ref() returns
            // (&InnerStream, &ServerConnection). ServerConnection
            // exposes peer_certificates() returning the client's
            // cert chain (None if mTLS not configured or no cert
            // presented).
            let (_, server_conn) = tls_stream.get_ref();
            let peer_certs = match server_conn.peer_certificates() {
                Some(certs) if !certs.is_empty() => {
                    let owned: Vec<CertificateDer<'static>> =
                        certs.iter().map(|c| c.clone().into_owned()).collect();
                    PeerCertificates::new(owned)
                }
                _ => PeerCertificates::empty(),
            };

            Ok((tls_stream, PeerCertService::new(inner_service, peer_certs)))
        })
    }
}

// =====================================================================
// PeerCertService — tower::Service wrapper that injects the extension
// =====================================================================

/// Per-connection [`tower::Service`] wrapper that injects
/// [`PeerCertificates`] into every request's extensions. Constructed
/// internally by [`MtlsAcceptor::accept`]; not meant to be built by
/// hand.
#[derive(Clone, Debug)]
pub struct PeerCertService<S> {
    inner: S,
    peer_certs: PeerCertificates,
}

impl<S> PeerCertService<S> {
    fn new(inner: S, peer_certs: PeerCertificates) -> Self {
        Self { inner, peer_certs }
    }
}

impl<S, B> tower_service::Service<http::Request<B>> for PeerCertService<S>
where
    S: tower_service::Service<http::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: http::Request<B>) -> Self::Future {
        req.extensions_mut().insert(self.peer_certs.clone());
        self.inner.call(req)
    }
}

// =====================================================================
// Middleware: CN must match the {id} path segment on agent routes
// =====================================================================

/// Middleware applied to agent-facing routes that take a `{id}` path
/// segment. Extracts the [`PeerCertificates`] injected by
/// [`MtlsAcceptor`], reads the leaf CN, and rejects with 403 if it
/// does not match the path id.
///
/// No-op when:
/// - The request has no `PeerCertificates` extension at all (e.g.
///   the integration test harness uses raw `axum::serve` over a TCP
///   listener with no TLS layer).
/// - The `PeerCertificates` extension is present but empty (mTLS is
///   not configured at the server level).
///
/// Both no-op cases let the request through unchanged so PR-1's
/// TLS-only mode keeps working. Agent-route wiring lands in PR-3
/// when `/v1/agent/checkin` and `/v1/agent/report` go in.
pub async fn cn_matches_path_machine_id(
    Path(params): Path<HashMap<String, String>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(id) = params.get("id") else {
        return Ok(next.run(request).await);
    };

    let Some(certs) = request.extensions().get::<PeerCertificates>() else {
        return Ok(next.run(request).await);
    };

    if !certs.is_present() {
        return Ok(next.run(request).await);
    }

    let cn = certs.leaf_cn().ok_or_else(|| {
        tracing::warn!(
            path_id = %id,
            "Rejecting agent request: peer certificate has no CN"
        );
        StatusCode::FORBIDDEN
    })?;

    if cn != id.as_str() {
        tracing::warn!(
            cert_cn = %cn,
            path_id = %id,
            "Rejecting agent request: cert CN does not match path machine_id"
        );
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_peer_certs_are_not_present() {
        let pc = PeerCertificates::empty();
        assert!(!pc.is_present());
        assert!(pc.leaf().is_none());
        assert!(pc.leaf_cn().is_none());
    }

    #[test]
    fn default_peer_certs_are_empty() {
        let pc = PeerCertificates::default();
        assert!(!pc.is_present());
    }
}
