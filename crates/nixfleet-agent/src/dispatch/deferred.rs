//! Switch-inhibitor deferral handler: profile is set, live switch was skipped,
//! no rollback fires. The dispatch row is parked CP-side until the operator
//! reboots and boot-recovery posts the retroactive confirm.

use nixfleet_proto::agent_wire::ReportEvent;

use nixfleet_agent::checkin_state::{write_last_deferred, LastDeferredRecord};
use nixfleet_agent::comms::Reporter;

use super::DispatchCtx;

pub(crate) async fn handle_deferred_pending_reboot<R: Reporter>(
    ctx: &DispatchCtx<'_, R>,
    component: String,
) {
    tracing::info!(
        target_closure = %ctx.target.closure_hash,
        component = %component,
        "agent: activation deferred to next boot — switch-inhibitor on critical component",
    );
    // Persist the sentinel BEFORE posting the event: if the post fails the
    // suppression still kicks in next poll (the next dispatch's re-post is
    // also idempotent on the CP side, so worst case the operator sees the
    // event one cycle later than the journal).
    let record = LastDeferredRecord {
        closure_hash: ctx.target.closure_hash.clone(),
        channel_ref: ctx.target.channel_ref.clone(),
        component: component.clone(),
        deferred_at: chrono::Utc::now(),
    };
    if let Err(err) = write_last_deferred(&ctx.args.state_dir, &record) {
        tracing::warn!(
            error = %err,
            state_dir = %ctx.args.state_dir.display(),
            "write_last_deferred failed (non-fatal); next poll will re-detect + re-post",
        );
    }
    ctx.reporter
        .post_report(
            Some(&ctx.target.channel_ref),
            ReportEvent::ActivationDeferred {
                closure_hash: ctx.target.closure_hash.clone(),
                channel_ref: ctx.target.channel_ref.clone(),
                component,
            },
        )
        .await;
}
