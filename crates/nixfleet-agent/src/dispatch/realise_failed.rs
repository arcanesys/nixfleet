//! Realise-step failure handlers: nothing was switched, neither rolls back.

use nixfleet_proto::agent_wire::ReportEvent;

use nixfleet_agent::comms::Reporter;

use super::DispatchCtx;

pub(crate) async fn handle_realise_failed<R: Reporter>(ctx: &DispatchCtx<'_, R>, reason: String) {
    tracing::warn!(
        reason = %reason,
        "activation: realise failed; nothing switched, retrying next tick",
    );
    let signature = ctx.try_sign(
        &nixfleet_agent::evidence_signer::RealiseFailedSignedPayload {
            hostname: &ctx.args.machine_id,
            rollout: Some(&ctx.target.channel_ref),
            closure_hash: &ctx.target.closure_hash,
            reason: &reason,
        },
    );
    ctx.reporter
        .post_report(
            Some(&ctx.target.channel_ref),
            ReportEvent::RealiseFailed {
                closure_hash: ctx.target.closure_hash.clone(),
                reason,
                signature,
            },
        )
        .await;
}

pub(crate) async fn handle_closure_signature_mismatch<R: Reporter>(
    ctx: &DispatchCtx<'_, R>,
    closure_hash: String,
    stderr_tail: String,
) {
    tracing::error!(
        closure_hash = %closure_hash,
        stderr_tail = %stderr_tail,
        "activation: closure signature mismatch - refused by nix substituter trust",
    );
    let stderr_tail_sha256 =
        nixfleet_agent::evidence_signer::sha256_jcs(&stderr_tail).unwrap_or_default();
    let signature = ctx.try_sign(
        &nixfleet_agent::evidence_signer::ClosureSignatureMismatchSignedPayload {
            hostname: &ctx.args.machine_id,
            rollout: Some(&ctx.target.channel_ref),
            closure_hash: &closure_hash,
            stderr_tail_sha256,
        },
    );
    ctx.reporter
        .post_report(
            Some(&ctx.target.channel_ref),
            ReportEvent::ClosureSignatureMismatch {
                closure_hash,
                stderr_tail,
                signature,
            },
        )
        .await;
}
