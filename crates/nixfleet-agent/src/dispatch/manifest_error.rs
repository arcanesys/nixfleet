//! Manifest-gate failure handler: emit signed event, do not proceed with target.
//! No rollback because nothing was activated.

use nixfleet_proto::agent_wire::ReportEvent;

use nixfleet_agent::comms::Reporter;
use nixfleet_agent::manifest_cache::ManifestError;

use super::DispatchCtx;

pub(crate) async fn handle<R: Reporter>(
    ctx: &DispatchCtx<'_, R>,
    err: ManifestError,
    rollout_id: &str,
) {
    let reason = err.reason().to_string();
    let kind = match err {
        ManifestError::Missing(_) => "missing",
        ManifestError::VerifyFailed(_) => "verify-failed",
        ManifestError::Mismatch(_) => "mismatch",
    };
    tracing::error!(
        rollout_id,
        kind,
        reason = %reason,
        "agent: refusing dispatch - rollout manifest gate failed",
    );

    let hostname = &ctx.args.machine_id;
    let event = match err {
        ManifestError::Missing(_) => {
            let signature = ctx.try_sign(
                &nixfleet_agent::evidence_signer::ManifestMissingSignedPayload {
                    hostname,
                    rollout: Some(rollout_id),
                    rollout_id,
                    reason: &reason,
                },
            );
            ReportEvent::ManifestMissing {
                rollout_id: rollout_id.to_string(),
                reason,
                signature,
            }
        }
        ManifestError::VerifyFailed(_) => {
            let signature = ctx.try_sign(
                &nixfleet_agent::evidence_signer::ManifestVerifyFailedSignedPayload {
                    hostname,
                    rollout: Some(rollout_id),
                    rollout_id,
                    reason: &reason,
                },
            );
            ReportEvent::ManifestVerifyFailed {
                rollout_id: rollout_id.to_string(),
                reason,
                signature,
            }
        }
        ManifestError::Mismatch(_) => {
            let signature = ctx.try_sign(
                &nixfleet_agent::evidence_signer::ManifestMismatchSignedPayload {
                    hostname,
                    rollout: Some(rollout_id),
                    rollout_id,
                    reason: &reason,
                },
            );
            ReportEvent::ManifestMismatch {
                rollout_id: rollout_id.to_string(),
                reason,
                signature,
            }
        }
    };

    ctx.reporter
        .post_report(Some(&ctx.target.channel_ref), event)
        .await;
}
