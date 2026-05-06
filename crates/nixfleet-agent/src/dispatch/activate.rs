//! Dispatch entry: freshness gate → manifest gate → activate → route outcome.

use std::sync::Arc;

use nixfleet_proto::agent_wire::{EvaluatedTarget, FetchOutcome, FetchResult, ReportEvent};
use nixfleet_proto::RolloutManifest;

use nixfleet_agent::comms::Reporter;
use nixfleet_agent::evidence_signer::EvidenceSigner;
use nixfleet_agent::manifest_cache::ManifestError;

use crate::Args;

use super::confirm::handle_fired_and_polled;
use super::DispatchCtx;
use super::manifest_error;
use super::realise_failed::{handle_closure_signature_mismatch, handle_realise_failed};
use super::verify_mismatch::{handle_switch_failed, handle_verify_mismatch};

/// Map a manifest-cache result onto the wire enum the CP circuit-breaker
/// understands. `Missing` is HTTP-shaped (404 / 5xx / network) → FetchFailed;
/// `VerifyFailed` and `Mismatch` are content-shaped → VerifyFailed.
fn fetch_outcome_for(result: &Result<RolloutManifest, ManifestError>) -> FetchOutcome {
    match result {
        Ok(_) => FetchOutcome {
            result: FetchResult::Ok,
            error: None,
        },
        Err(ManifestError::Missing(s)) => FetchOutcome {
            result: FetchResult::FetchFailed,
            error: Some(s.clone()),
        },
        Err(ManifestError::VerifyFailed(s)) | Err(ManifestError::Mismatch(s)) => FetchOutcome {
            result: FetchResult::VerifyFailed,
            error: Some(s.clone()),
        },
    }
}

pub(crate) async fn process_dispatch_target(
    target: &EvaluatedTarget,
    reporter: &impl Reporter,
    client: &reqwest::Client,
    args: &Args,
    evidence_signer: &Arc<Option<EvidenceSigner>>,
) {
    let ctx = DispatchCtx {
        target,
        reporter,
        args,
        evidence_signer,
    };
    use nixfleet_agent::freshness::{check as freshness_check, FreshnessCheck};
    if let FreshnessCheck::Stale {
        signed_at,
        freshness_window_secs,
        age_secs,
    } = freshness_check(target, chrono::Utc::now())
    {
        tracing::warn!(
            closure_hash = %target.closure_hash,
            channel_ref = %target.channel_ref,
            signed_at = %signed_at,
            freshness_window_secs,
            age_secs,
            "agent: refusing stale target — fleet.resolved older than freshness_window + 60s slack",
        );
        let stale_payload = nixfleet_agent::evidence_signer::StaleTargetSignedPayload {
            hostname: &args.machine_id,
            rollout: Some(&target.channel_ref),
            closure_hash: &target.closure_hash,
            channel_ref: &target.channel_ref,
            signed_at,
            freshness_window_secs,
            age_secs,
        };
        let signature = ctx.try_sign(&stale_payload);
        reporter
            .post_report(
                Some(&target.channel_ref),
                ReportEvent::StaleTarget {
                    closure_hash: target.closure_hash.clone(),
                    channel_ref: target.channel_ref.clone(),
                    signed_at,
                    freshness_window_secs,
                    age_secs,
                    signature,
                },
            )
            .await;
        return;
    }

    // LOADBEARING: verify manifest + membership BEFORE consuming any target field — refuse-to-act.
    let rollout_id = target.rollout_id.as_str();
    let cache = nixfleet_agent::manifest_cache::ManifestCache::new(
        &args.state_dir,
        &args.trust_file,
    );
    let wave_index = target.wave_index.unwrap_or(0);
    let fetch_result = cache
        .ensure(client, &args.control_plane_url, rollout_id, &args.machine_id, wave_index)
        .await;
    // Persist outcome BEFORE any branch returns — CP's circuit breaker
    // (Decision::HoldAfterFailure) reads this on the next checkin.
    let _ = nixfleet_agent::checkin_state::write_last_fetch_outcome(
        &args.state_dir,
        &fetch_outcome_for(&fetch_result),
    );
    match fetch_result {
        Ok(_manifest) => {
            tracing::debug!(
                rollout_id = %rollout_id,
                wave_index = wave_index,
                "agent: rollout manifest verified",
            );
        }
        Err(err) => {
            manifest_error::handle(&ctx, err, rollout_id).await;
            return;
        }
    }

    // Boot-recovery is the retroactive-confirm path; for non-confirmable
    // targets (no activate block) there's no recovery work, so skip the
    // write entirely. GOTCHA: write failure only loses boot-recovery —
    // next-checkin re-dispatches.
    if let Some(activate) = target.activate.as_ref() {
        let dispatch_record = nixfleet_agent::checkin_state::LastDispatchRecord {
            closure_hash: target.closure_hash.clone(),
            channel_ref: target.channel_ref.clone(),
            rollout_id: target.rollout_id.clone(),
            compliance_mode: target.compliance_mode.clone(),
            confirm_endpoint: activate.confirm_endpoint.clone(),
            dispatched_at: chrono::Utc::now(),
        };
        if let Err(err) = nixfleet_agent::checkin_state::write_last_dispatched(
            &args.state_dir,
            &dispatch_record,
        ) {
            tracing::warn!(
                error = %err,
                state_dir = %args.state_dir.display(),
                "write_last_dispatched failed; boot-recovery path will fall back to next-checkin re-dispatch",
            );
        }
    }

    reporter
        .post_report(
            Some(&target.channel_ref),
            ReportEvent::ActivationStarted {
                closure_hash: target.closure_hash.clone(),
                channel_ref: target.channel_ref.clone(),
            },
        )
        .await;

    let outcome = nixfleet_agent::activation::activate(target).await;
    handle_activation_outcome(outcome, &ctx, client).await;
}

async fn handle_activation_outcome<R: Reporter>(
    outcome: anyhow::Result<nixfleet_agent::activation::ActivationOutcome>,
    ctx: &DispatchCtx<'_, R>,
    client_handle: &reqwest::Client,
) {
    use nixfleet_agent::activation::ActivationOutcome;
    match outcome {
        Ok(ActivationOutcome::FiredAndPolled) => handle_fired_and_polled(ctx, client_handle).await,
        Ok(ActivationOutcome::RealiseFailed { reason }) => handle_realise_failed(ctx, reason).await,
        Ok(ActivationOutcome::SignatureMismatch {
            closure_hash,
            stderr_tail,
        }) => handle_closure_signature_mismatch(ctx, closure_hash, stderr_tail).await,
        Ok(ActivationOutcome::SwitchFailed { phase, exit_code }) => {
            handle_switch_failed(ctx, phase, exit_code).await
        }
        Ok(ActivationOutcome::VerifyMismatch { expected, actual }) => {
            handle_verify_mismatch(ctx, expected, actual).await
        }
        Err(err) => handle_activation_spawn_error(ctx, err).await,
    }
}

/// State unknown (may have failed pre-realise) so no rollback; posts unsigned `Other`.
async fn handle_activation_spawn_error<R: Reporter>(ctx: &DispatchCtx<'_, R>, err: anyhow::Error) {
    tracing::error!(error = %err, "activation spawn failed");
    ctx.reporter
        .post_report(
            Some(&ctx.target.channel_ref),
            ReportEvent::Other {
                kind: "activation-spawn-failed".to_string(),
                detail: Some(serde_json::json!({
                    "error": err.to_string(),
                    "target_closure": ctx.target.closure_hash,
                })),
            },
        )
        .await;
}
