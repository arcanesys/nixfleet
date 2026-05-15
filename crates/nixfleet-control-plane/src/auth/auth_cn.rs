//! mTLS peer-cert extraction; injects chain as a per-request extension.
//!
//! FOOTGUN: `axum-server 0.7` does not expose peer certificates publicly.
//! The `MtlsAcceptor` wrapper reads them post-handshake from the rustls
//! TlsStream and injects via per-connection tower::Service. Don't remove
//! without a replacement - the chain is otherwise unreachable.

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

/// Empty when no client cert was presented.
#[derive(Clone, Debug, Default)]
pub struct PeerCertificates {
    /// DER, leaf first.
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

    pub fn leaf_cn(&self) -> Option<String> {
        let leaf = self.leaf()?;
        let (_, cert) = X509Certificate::from_der(leaf.as_ref()).ok()?;
        // Bind locally so the x509-parser temporary drops first.

        cert.subject()
            .iter_common_name()
            .next()
            .and_then(|attr| attr.as_str().ok().map(String::from))
    }

    /// LOADBEARING: revocations are "notBefore < X is bad" - re-enrolling
    /// (notBefore > X) re-grants access; don't change to issuance time.
    pub fn leaf_not_before(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        let leaf = self.leaf()?;
        let (_, cert) = X509Certificate::from_der(leaf.as_ref()).ok()?;
        let secs = cert.validity().not_before.timestamp();
        chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
    }
}

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
            let (tls_stream, inner_service) = inner.accept(stream, service).await?;

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

/// 403 if leaf CN doesn't match `{id}`; no-op when extension is absent or empty.
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
