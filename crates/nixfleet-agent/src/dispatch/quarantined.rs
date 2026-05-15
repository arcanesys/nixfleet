//! Quarantine-suppression handler (issue #55): the dispatch loop calls
//! `should_suppress_quarantined_dispatch` before activate() to decide whether
//! to skip a closure that already failed within the quarantine window.
//!
//! Skip is paired with a throttled `ClosureQuarantined` event post: the first
//! suppression fires the event, subsequent suppressions within
//! `QUARANTINE_REPOST_THROTTLE_SECS` are silent. `failure_count` tracks
//! distinct switch failures, NOT suppression hits - flapping the event log
//! on every poll would flood the operator's dashboard.

use chrono::{DateTime, Utc};
use nixfleet_proto::agent_wire::{EvaluatedTarget, ReportEvent};

use nixfleet_agent::checkin_state::{
    LastFailedClosureRecord, QUARANTINE_REPOST_THROTTLE_SECS, QUARANTINE_WINDOW_SECS,
    read_last_failed_closure, write_last_failed_closure,
};
use nixfleet_agent::comms::Reporter;

use super::DispatchCtx;

/// Outcome of the suppression check; `Suppress` carries the recorded
/// failure context so the caller can post the quarantine event without
/// re-reading the state-dir.
pub(crate) enum QuarantineDecision {
    Proceed,
    Suppress(LastFailedClosureRecord),
}

/// Decides whether to short-circuit the dispatch. Suppression matches when
/// the recorded `closure_hash` equals `target.closure_hash` AND the failure
/// is recent (within `QUARANTINE_WINDOW_SECS`). Read failures are fail-open
/// (Proceed) - the agent will re-attempt and either succeed or re-record
/// the failure.
pub(crate) fn evaluate(
    state_dir: &std::path::Path,
    target: &EvaluatedTarget,
    now: DateTime<Utc>,
) -> QuarantineDecision {
    let Ok(Some(record)) = read_last_failed_closure(state_dir) else {
        return QuarantineDecision::Proceed;
    };
    if record.closure_hash != target.closure_hash {
        return QuarantineDecision::Proceed;
    }
    let age = now.signed_duration_since(record.last_failure_at);
    if age.num_seconds() > QUARANTINE_WINDOW_SECS {
        return QuarantineDecision::Proceed;
    }
    QuarantineDecision::Suppress(record)
}

/// Post `ClosureQuarantined` if we haven't recently, then mark the post
/// timestamp on the record. Throttled to one post per
/// `QUARANTINE_REPOST_THROTTLE_SECS` to bound journal volume during
/// steady-state quarantine.
pub(crate) async fn post_quarantine_event<R: Reporter>(
    ctx: &DispatchCtx<'_, R>,
    mut record: LastFailedClosureRecord,
    now: DateTime<Utc>,
) {
    let should_post = match record.last_quarantine_post_at {
        None => true,
        Some(ts) => now.signed_duration_since(ts).num_seconds() >= QUARANTINE_REPOST_THROTTLE_SECS,
    };

    tracing::info!(
        target_closure = %ctx.target.closure_hash,
        failure_count = record.failure_count,
        will_post = should_post,
        "agent: skipping dispatch - closure quarantined after prior activation failure",
    );

    if !should_post {
        return;
    }

    ctx.reporter
        .post_report(
            Some(&ctx.target.channel_ref),
            ReportEvent::ClosureQuarantined {
                closure_hash: ctx.target.closure_hash.clone(),
                channel_ref: ctx.target.channel_ref.clone(),
                failure_count: record.failure_count,
                reason: record.reason.clone(),
            },
        )
        .await;

    record.last_quarantine_post_at = Some(now);
    if let Err(err) = write_last_failed_closure(&ctx.args.state_dir, &record) {
        tracing::warn!(
            error = %err,
            state_dir = %ctx.args.state_dir.display(),
            "write_last_failed_closure update failed; throttle window may not advance",
        );
    }
}
