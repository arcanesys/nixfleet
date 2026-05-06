//! Long-running TLS server: router + listener + reconcile loop + polls.

mod checkin_pipeline;
mod middleware;
mod reconcile;
mod routes;
mod state;

pub use state::{
    AppState, ClosureUpstream, HostCheckinRecord, IssuancePaths, ReportRecord, ServeArgs,
    VerifiedFleetSnapshot,
};

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request as HttpRequest;
use axum::middleware::Next;
use axum::routing::{get, post};
use axum::Router;
use chrono::Utc;
use tokio_util::sync::CancellationToken;

use crate::auth::auth_cn::MtlsAcceptor;
use crate::TickInputs;

/// 30s = systemd `TimeoutStopSec=` default; stay under to avoid SIGKILL.
const TASK_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(30);

/// Listener drain budget; remaining 5s of `TASK_SHUTDOWN_DEADLINE` is for background drain.
const HTTP_DRAIN_DEADLINE: Duration = Duration::from_secs(25);

/// `/healthz` outside `/v1`; `/v1/enroll` is anonymous; all other `/v1/*` require mTLS.
fn build_router(state: Arc<AppState>) -> Router {
    let strict = state.strict;
    let auth_state = state.clone();

    let anonymous_v1 = Router::new()
        .route("/v1/enroll", post(routes::enrollment::enroll))
        .route(
            "/v1/agent/bootstrap-report",
            post(routes::bootstrap_report::bootstrap_report),
        );

    let authenticated_v1 = Router::new()
        .route("/v1/whoami", get(routes::status::whoami))
        .route("/v1/agent/checkin", post(checkin_pipeline::checkin))
        .route("/v1/agent/report", post(routes::reports::report))
        .route("/v1/host-reports", get(routes::reports::list_recent))
        .route("/v1/agent/confirm", post(checkin_pipeline::confirm))
        .route(
            "/v1/agent/closure/{hash}",
            get(routes::status::closure_proxy),
        )
        .route("/v1/agent/renew", post(routes::enrollment::renew))
        .route("/v1/channels/{name}", get(routes::status::channel_status))
        .route("/v1/hosts", get(routes::status::hosts_status))
        .route("/metrics", get(routes::metrics::metrics_handler))
        .route("/v1/rollouts", get(routes::rollouts::list_active))
        .route("/v1/deferrals", get(routes::deferrals::list))
        .route("/v1/rollouts/{rolloutId}", get(routes::rollouts::manifest))
        .route(
            "/v1/rollouts/{rolloutId}/sig",
            get(routes::rollouts::signature),
        )
        .route(
            "/v1/rollouts/{rolloutId}/lifecycle",
            get(routes::rollouts::lifecycle),
        )
        .route(
            "/v1/rollouts/{rolloutId}/trace",
            get(routes::rollouts::trace),
        )
        .layer(axum::middleware::from_fn(move |req, next| {
            let s = auth_state.clone();
            async move { middleware::require_cn_layer(s, req, next).await }
        }));

    let v1_routes = anonymous_v1
        .merge(authenticated_v1)
        .layer(axum::middleware::from_fn(move |req, next| {
            version_layer(strict, req, next)
        }));

    Router::new()
        .route("/healthz", get(routes::health::healthz))
        .merge(v1_routes)
        .with_state(state)
}

async fn version_layer(
    strict: bool,
    req: HttpRequest<Body>,
    next: Next,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    middleware::protocol_version_middleware(strict, req, next).await
}

pub async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    if args.strict {
        let mut missing: Vec<&str> = Vec::new();
        if args.client_ca.is_none() {
            missing.push("--client-ca (mTLS verification disabled — TLS-only mode)");
        }
        if args.revocations.is_none() {
            missing.push("--revocations-{artifact,signature}-url (revocations polling disabled — previously-revoked certs become valid again after CP rebuild)");
        }
        if !missing.is_empty() {
            anyhow::bail!(
                "--strict refuses to start: the following security flags are unset:\n  - {}\n\
                 Either set the missing flags or drop --strict for development.",
                missing.join("\n  - "),
            );
        }
    }

    let db = if let Some(path) = &args.db_path {
        let db = crate::db::Db::open(path)?;
        db.migrate()?;
        tracing::info!(path = %path.display(), "sqlite opened + migrated");
        Some(Arc::new(db))
    } else {
        None
    };

    let closure_upstream = if let Some(base_url) = &args.closure_upstream {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| anyhow::anyhow!("build closure proxy client: {e}"))?;
        Some(ClosureUpstream {
            base_url: base_url.clone(),
            client,
        })
    } else {
        None
    };
    let app_state = AppState {
        db: db.clone(),
        confirm_deadline_secs: args.confirm_deadline_secs,
        closure_upstream,
        rollouts_dir: args.rollouts_dir.clone(),
        rollouts_source: args.rollouts_source.clone(),
        strict: args.strict,
        ..Default::default()
    };
    let state = Arc::new(app_state);

    let cancel = CancellationToken::new();
    let mut bg_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    if let Some(db_arc) = db.clone() {
        bg_handles.push(crate::timers::rollback_timer::spawn(
            cancel.clone(),
            db_arc.clone(),
        ));
        bg_handles.push(crate::timers::prune_timer::spawn(
            cancel.clone(),
            db_arc,
            args.db_path.clone(),
        ));
    }

    // Hydrate the host_reports ring at boot so wave-gate state survives CP restart.
    if let Some(db_arc) = db.clone() {
        match db_arc.reports().host_reports_known_hostnames() {
            Ok(hostnames) => {
                let mut total = 0usize;
                let mut reports_w = state.host_reports.write().await;
                for hostname in &hostnames {
                    match db_arc.reports().host_reports_recent_per_host(
                        hostname,
                        crate::server::state::REPORT_RING_CAP,
                    ) {
                        Ok(rows) => {
                            for row in rows {
                                let req: nixfleet_proto::agent_wire::ReportRequest =
                                    match serde_json::from_str(&row.report_json) {
                                        Ok(r) => r,
                                        Err(err) => {
                                            tracing::warn!(
                                                hostname = %hostname,
                                                event_id = %row.event_id,
                                                error = %err,
                                                "host_reports hydration: unparseable row, skipping"
                                            );
                                            continue;
                                        }
                                    };
                                let signature_status = row.signature_status.and_then(|s| {
                                    serde_json::from_value::<
                                        nixfleet_reconciler::evidence::SignatureStatus,
                                    >(serde_json::Value::String(
                                        s,
                                    ))
                                    .ok()
                                });
                                let buf = reports_w.entry(hostname.clone()).or_default();
                                buf.push_back(crate::server::ReportRecord {
                                    event_id: row.event_id,
                                    received_at: row.received_at,
                                    report: req,
                                    signature_status,
                                });
                                total += 1;
                            }
                        }
                        Err(err) => {
                            tracing::warn!(
                                hostname = %hostname,
                                error = %err,
                                "host_reports hydration: per-host query failed",
                            );
                        }
                    }
                }
                tracing::info!(
                    target: "boot",
                    hosts = hostnames.len(),
                    rows_loaded = total,
                    "host_reports hydration complete",
                );
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "host_reports hydration: failed to enumerate hostnames; ring buffer starts empty",
                );
            }
        }
    }

    *state.issuance_paths.write().await = IssuancePaths {
        fleet_ca_cert: args.fleet_ca_cert.clone(),
        fleet_ca_key: args.fleet_ca_key.clone(),
        audit_log: args.audit_log_path.clone(),
    };

    // Pre-listener prime: bundled artifact is always older than upstream (CI commits release after build).
    if let Some(channel_refs_source) = args.channel_refs.as_ref() {
        match tokio::time::timeout(
            Duration::from_secs(20),
            crate::polling::channel_refs_poll::prime_once(channel_refs_source),
        )
        .await
        {
            Ok(Ok((fleet, fleet_hash))) => {
                let host_count = fleet.hosts.len();
                *state.verified_fleet.write().await =
                    Some(crate::server::VerifiedFleetSnapshot {
                        fleet: Arc::new(fleet),
                        fleet_resolved_hash: fleet_hash,
                    });
                tracing::info!(
                    target: "reconcile",
                    host_count,
                    "primed verified-fleet from channel-refs source before opening listener",
                );
            }
            Ok(Err(err)) => {
                tracing::warn!(
                    error = %err,
                    "channel-refs prime failed; falling back to build-time artifact",
                );
            }
            Err(_) => {
                tracing::warn!("channel-refs prime timed out; falling back to build-time artifact",);
            }
        }
    }

    let tick_inputs = TickInputs {
        artifact_path: args.artifact_path.clone(),
        signature_path: args.signature_path.clone(),
        trust_path: args.trust_path.clone(),
        observed_path: args.observed_path.clone(),
        now: Utc::now(),
        freshness_window: args.freshness_window,
    };
    bg_handles.push(reconcile::spawn_reconcile_loop(
        cancel.clone(),
        state.clone(),
        tick_inputs,
    ));

    if let Some(channel_refs_source) = args.channel_refs.clone() {
        bg_handles.push(crate::polling::channel_refs_poll::spawn(
            cancel.clone(),
            state.channel_refs_cache.clone(),
            state.verified_fleet.clone(),
            state.db.clone(),
            state.last_deferrals.clone(),
            channel_refs_source,
            // Reconciler-driven event kick — closes the timing window
            // between channelEdges releasing and the rollouts table
            // reflecting the new successor. See AppState::channel_refs_kick.
            Some(state.channel_refs_kick.subscribe()),
        ));
    }

    if let (Some(revocations_source), Some(db)) = (args.revocations.clone(), state.db.clone()) {
        bg_handles.push(crate::polling::revocations_poll::spawn(
            cancel.clone(),
            db,
            revocations_source,
        ));
    }

    // Install Prometheus recorder before any request handler can call
    // gauge!/counter! macros. Idempotent — first install wins per process.
    // `cp_build_info` is re-emitted from `record_fleet_metrics` each scrape
    // so the gauge idle-timeout doesn't evict it.
    crate::metrics::install_recorder();

    let app = build_router(state);

    let tls_config =
        crate::tls::build_server_config(&args.tls_cert, &args.tls_key, args.client_ca.as_deref())?;
    let rustls_config = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(tls_config));

    let rustls_acceptor = axum_server::tls_rustls::RustlsAcceptor::new(rustls_config);
    let mtls_acceptor = MtlsAcceptor::new(rustls_acceptor);

    let mode = if args.client_ca.is_some() {
        "TLS+mTLS"
    } else {
        tracing::warn!(
            "control plane started without --client-ca: /v1/* endpoints will reject all clients with 401. \
             Pass --client-ca to enable mTLS — recommended for any production deployment."
        );
        "TLS-only"
    };
    tracing::info!(listen = %args.listen, %mode, "control plane listening");

    let server_handle = axum_server::Handle::new();
    let signal_handle = server_handle.clone();
    let signal_cancel = cancel.clone();
    // LOADBEARING: graceful_shutdown FIRST (drains in-flight HTTP), THEN
    // cancel the token (signals background tasks). Reversing causes timers
    // to abort mid-write while requests are still completing.
    tokio::spawn(async move {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %err, "ctrl_c handler install failed; relying on hard shutdown");
            return;
        }
        tracing::info!(target: "shutdown", "graceful shutdown initiated");
        signal_handle.graceful_shutdown(Some(HTTP_DRAIN_DEADLINE));
        signal_cancel.cancel();
    });

    axum_server::bind(args.listen)
        .acceptor(mtls_acceptor)
        .handle(server_handle)
        .serve(app.into_make_service())
        .await?;

    cancel.cancel();
    if let Err(err) = drain_background_tasks(bg_handles).await {
        tracing::warn!(error = %err, "background task drain incomplete");
    }
    Ok(())
}

/// Tasks past `TASK_SHUTDOWN_DEADLINE` are abandoned (handles dropped → abort).
async fn drain_background_tasks(
    handles: Vec<tokio::task::JoinHandle<()>>,
) -> anyhow::Result<()> {
    let total = handles.len();
    let drain_fut = async move {
        for handle in handles {
            if let Err(err) = handle.await {
                if !err.is_cancelled() {
                    tracing::warn!(error = %err, "background task panicked during shutdown");
                }
            }
        }
    };
    match tokio::time::timeout(TASK_SHUTDOWN_DEADLINE, drain_fut).await {
        Ok(()) => {
            tracing::info!(target: "shutdown", tasks = total, "all background tasks shut down");
            Ok(())
        }
        Err(_) => {
            anyhow::bail!(
                "background task drain exceeded {TASK_SHUTDOWN_DEADLINE:?}; forcing exit"
            );
        }
    }
}

#[cfg(test)]
mod strict_mode_tests {
    use super::*;
    use std::path::PathBuf;

    fn minimal_serve_args(strict: bool, client_ca: Option<PathBuf>) -> ServeArgs {
        ServeArgs {
            tls_cert: PathBuf::from("/dev/null"),
            tls_key: PathBuf::from("/dev/null"),
            client_ca,
            artifact_path: PathBuf::from("/dev/null"),
            signature_path: PathBuf::from("/dev/null"),
            trust_path: PathBuf::from("/dev/null"),
            observed_path: PathBuf::from("/dev/null"),
            strict,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn strict_bails_when_client_ca_unset() {
        let err = serve(minimal_serve_args(true, None)).await.unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("--client-ca"),
            "expected client-ca hint in strict bail; got: {msg}",
        );
        assert!(
            msg.contains("--strict refuses to start"),
            "expected strict-prefixed message; got: {msg}",
        );
    }

    #[tokio::test]
    async fn strict_bails_when_revocations_unset() {
        let err = serve(minimal_serve_args(true, Some(PathBuf::from("/dev/null"))))
            .await
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("--revocations"),
            "expected revocations hint in strict bail; got: {msg}",
        );
    }

    #[tokio::test]
    async fn non_strict_does_not_bail_at_startup() {
        let err = serve(minimal_serve_args(false, None)).await.unwrap_err();
        let msg = format!("{err}");
        assert!(
            !msg.contains("--strict refuses to start"),
            "non-strict mode should not emit the strict-mode error; got: {msg}",
        );
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn drain_returns_ok_when_tasks_exit_promptly() {
        let cancel = CancellationToken::new();
        let handles: Vec<_> = (0..3)
            .map(|_| {
                let c = cancel.clone();
                tokio::spawn(async move {
                    c.cancelled().await;
                })
            })
            .collect();

        cancel.cancel();
        drain_background_tasks(handles)
            .await
            .expect("tasks should drain cleanly");
    }

    #[tokio::test(start_paused = true)]
    async fn drain_bails_when_task_ignores_cancel() {
        let cancel = CancellationToken::new();
        let stuck = tokio::spawn(async {
            tokio::time::sleep(TASK_SHUTDOWN_DEADLINE + Duration::from_secs(60)).await;
        });
        let handles = vec![stuck];

        cancel.cancel();
        let drain = tokio::spawn(async move { drain_background_tasks(handles).await });
        tokio::time::advance(TASK_SHUTDOWN_DEADLINE + Duration::from_secs(1)).await;
        let err = drain.await.unwrap().unwrap_err();
        assert!(
            err.to_string().contains("forcing exit"),
            "expected force-exit message; got: {err}",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_token_unblocks_select_loop() {
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(3600));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = task_cancel.cancelled() => return,
                    _ = ticker.tick() => {}
                }
            }
        });

        tokio::task::yield_now().await;
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("task should exit on cancel within 5s")
            .expect("task should not panic");
    }
}
