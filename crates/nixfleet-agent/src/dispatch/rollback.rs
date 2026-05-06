//! CP-driven rollback per `CheckinResponse.rollback`; idempotent (CP re-emits
//! until the agent's `RollbackTriggered` flips state to `Reverted`).

use nixfleet_proto::agent_wire::ReportEvent;

use nixfleet_agent::comms::Reporter;

use crate::Args;

use nixfleet_agent::evidence_signer::try_sign;

pub(crate) async fn handle_cp_rollback_signal(
    rb: &nixfleet_proto::agent_wire::RollbackSignal,
    reporter: &impl Reporter,
    args: &Args,
    evidence_signer: &std::sync::Arc<Option<nixfleet_agent::evidence_signer::EvidenceSigner>>,
) {
    tracing::warn!(
        rollout = %rb.rollout,
        target_ref = %rb.target_ref,
        reason = %rb.reason,
        "agent: CP issued rollback signal (rollback-and-halt policy); rolling back",
    );
    let rb_outcome = nixfleet_agent::activation::rollback().await;
    let reason = rb.reason.clone();
    let rollback_payload = nixfleet_agent::evidence_signer::RollbackTriggeredSignedPayload {
        hostname: &args.machine_id,
        rollout: Some(&rb.rollout),
        reason: &reason,
    };
    let signature = evidence_signer
        .as_ref()
        .as_ref()
        .and_then(|s| try_sign(s, &rollback_payload));
    reporter
        .post_report(
            Some(&rb.rollout),
            ReportEvent::RollbackTriggered { reason, signature },
        )
        .await;
    match &rb_outcome {
        Ok(o) if o.success() => {}
        Ok(o) => tracing::error!(
            phase = ?o.phase(),
            exit_code = ?o.exit_code(),
            "agent: CP-signalled rollback failed (poll/fire layer)",
        ),
        Err(err) => tracing::error!(
            error = %err,
            "agent: CP-signalled rollback transport-failed",
        ),
    }
}
