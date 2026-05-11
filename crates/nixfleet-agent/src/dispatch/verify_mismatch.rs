//! Switch / post-switch-verify failures: emit failure event, rollback, emit
//! follow-up event whose shape depends on the rollback outcome.

use nixfleet_proto::agent_wire::ReportEvent;

use nixfleet_agent::comms::Reporter;

use super::DispatchCtx;

/// Shared by `handle_switch_failed` + `handle_verify_mismatch`; arms map:
/// success → `RollbackTriggered`, partial-fail → `ActivationFailed{prefix/poll}`,
/// transport-err → `ActivationFailed{prefix, stderr_tail: err}`.
pub(super) fn compose_rollback_followup_event<R: Reporter>(
    rb_outcome: &anyhow::Result<nixfleet_agent::activation::RollbackOutcome>,
    ctx: &DispatchCtx<'_, R>,
    success_reason: String,
    failure_phase_prefix: &str,
) -> ReportEvent {
    match rb_outcome {
        Ok(o) if o.success() => {
            let signature = ctx.try_sign(
                &nixfleet_agent::evidence_signer::RollbackTriggeredSignedPayload {
                    hostname: &ctx.args.machine_id,
                    rollout: Some(&ctx.target.channel_ref),
                    reason: &success_reason,
                },
            );
            ReportEvent::RollbackTriggered {
                reason: success_reason,
                signature,
            }
        }
        Ok(o) => {
            let phase_str = format!("{failure_phase_prefix}/{}", o.phase().unwrap_or("unknown"));
            let exit = o.exit_code();
            let stderr_tail_sha256 =
                nixfleet_agent::evidence_signer::sha256_jcs(&"").unwrap_or_default();
            let signature = ctx.try_sign(
                &nixfleet_agent::evidence_signer::ActivationFailedSignedPayload {
                    hostname: &ctx.args.machine_id,
                    rollout: Some(&ctx.target.channel_ref),
                    phase: &phase_str,
                    exit_code: exit,
                    stderr_tail_sha256,
                },
            );
            ReportEvent::ActivationFailed {
                phase: phase_str,
                exit_code: exit,
                stderr_tail: None,
                signature,
            }
        }
        Err(err) => {
            let phase_str = failure_phase_prefix.to_string();
            let stderr_tail = err.to_string();
            let stderr_tail_sha256 =
                nixfleet_agent::evidence_signer::sha256_jcs(&stderr_tail).unwrap_or_default();
            let signature = ctx.try_sign(
                &nixfleet_agent::evidence_signer::ActivationFailedSignedPayload {
                    hostname: &ctx.args.machine_id,
                    rollout: Some(&ctx.target.channel_ref),
                    phase: &phase_str,
                    exit_code: None,
                    stderr_tail_sha256,
                },
            );
            ReportEvent::ActivationFailed {
                phase: phase_str,
                exit_code: None,
                stderr_tail: Some(stderr_tail),
                signature,
            }
        }
    }
}

pub(crate) async fn handle_switch_failed<R: Reporter>(
    ctx: &DispatchCtx<'_, R>,
    phase: String,
    exit_code: Option<i32>,
) {
    tracing::error!(
        phase = %phase,
        exit_code = ?exit_code,
        "activation: switch failed; rolling back",
    );
    // Issue #55: record the failure so the next dispatch for this same
    // closure_hash hits the quarantine suppression instead of repeating the
    // SwitchFailed → rollback cycle. Best-effort; a write failure only
    // means the next attempt will run normally and probably fail again,
    // which is the existing behavior.
    let reason = match exit_code {
        Some(code) => format!("phase={phase} exit={code}"),
        None => format!("phase={phase}"),
    };
    if let Err(err) = nixfleet_agent::checkin_state::record_switch_failure(
        &ctx.args.state_dir,
        &ctx.target.closure_hash,
        &ctx.target.channel_ref,
        &reason,
        chrono::Utc::now(),
    ) {
        tracing::warn!(
            error = %err,
            state_dir = %ctx.args.state_dir.display(),
            "record_switch_failure failed (non-fatal); next dispatch will not be quarantined",
        );
    }
    let stderr_tail_sha256 = nixfleet_agent::evidence_signer::sha256_jcs(&"").unwrap_or_default();
    let signature = ctx.try_sign(
        &nixfleet_agent::evidence_signer::ActivationFailedSignedPayload {
            hostname: &ctx.args.machine_id,
            rollout: Some(&ctx.target.channel_ref),
            phase: &phase,
            exit_code,
            stderr_tail_sha256,
        },
    );
    ctx.reporter
        .post_report(
            Some(&ctx.target.channel_ref),
            ReportEvent::ActivationFailed {
                phase: phase.clone(),
                exit_code,
                stderr_tail: None,
                signature,
            },
        )
        .await;
    let rb_outcome = nixfleet_agent::activation::rollback().await;
    let rollback_event = compose_rollback_followup_event(
        &rb_outcome,
        ctx,
        format!("activation phase {phase} failed"),
        &format!("rollback-after-{phase}"),
    );
    ctx.reporter
        .post_report(Some(&ctx.target.channel_ref), rollback_event)
        .await;
    if let Err(err) = rb_outcome {
        tracing::error!(
            error = %err,
            "rollback after failed switch also failed - manual intervention required",
        );
    }
}

pub(crate) async fn handle_verify_mismatch<R: Reporter>(
    ctx: &DispatchCtx<'_, R>,
    expected: String,
    actual: String,
) {
    tracing::error!(
        expected = %expected,
        actual = %actual,
        "activation: post-switch verify caught flip to unexpected closure; rolling back",
    );
    // Issue #55: same as SwitchFailed - record so the next dispatch
    // suppresses retry of this broken closure_hash.
    if let Err(err) = nixfleet_agent::checkin_state::record_switch_failure(
        &ctx.args.state_dir,
        &ctx.target.closure_hash,
        &ctx.target.channel_ref,
        &format!("verify-mismatch expected={expected} actual={actual}"),
        chrono::Utc::now(),
    ) {
        tracing::warn!(
            error = %err,
            state_dir = %ctx.args.state_dir.display(),
            "record_switch_failure failed (non-fatal); next dispatch will not be quarantined",
        );
    }
    let signature = ctx.try_sign(
        &nixfleet_agent::evidence_signer::VerifyMismatchSignedPayload {
            hostname: &ctx.args.machine_id,
            rollout: Some(&ctx.target.channel_ref),
            expected: &expected,
            actual: &actual,
        },
    );
    ctx.reporter
        .post_report(
            Some(&ctx.target.channel_ref),
            ReportEvent::VerifyMismatch {
                expected: expected.clone(),
                actual: actual.clone(),
                signature,
            },
        )
        .await;
    let rb_outcome = nixfleet_agent::activation::rollback().await;
    let rollback_event = compose_rollback_followup_event(
        &rb_outcome,
        ctx,
        format!("post-switch verify mismatch (expected {expected}, got {actual})"),
        "rollback-after-verify-mismatch",
    );
    ctx.reporter
        .post_report(Some(&ctx.target.channel_ref), rollback_event)
        .await;
    if let Err(err) = rb_outcome {
        tracing::error!(
            error = %err,
            "rollback after verify mismatch also failed - manual intervention required",
        );
    }
}
