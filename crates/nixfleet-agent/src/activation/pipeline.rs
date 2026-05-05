//! Main activate pipeline: realise -> set-profile -> fire -> poll -> self-correct.

use anyhow::{Context, Result};
use nixfleet_proto::agent_wire::EvaluatedTarget;
use tokio::process::Command;

use super::profile::self_correct_profile;
use super::realise::{realise, RealiseError};
use super::types::ActivationBackend;
use super::types::ActivationOutcome;
use super::verify_poll::{read_current_system_basename, PollOutcome, VerifyPoll};

/// Tests inject a fake backend; production calls the `activate(target)` façade.
pub async fn activate_with<B: ActivationBackend>(
    backend: &B,
    target: &EvaluatedTarget,
) -> Result<ActivationOutcome> {
    tracing::info!(
        target_closure = %target.closure_hash,
        target_channel = %target.channel_ref,
        "agent: activating target",
    );

    // GOTCHA: racing an in-flight switch yields spurious SwitchFailed timeouts even on success.
    if backend.is_switch_in_progress().await {
        tracing::info!(
            target_closure = %target.closure_hash,
            "agent: skipping activation - another switch-to-configuration is in flight",
        );
        return Ok(ActivationOutcome::RealiseFailed {
            reason: "switch-to-configuration lock held by another process; will retry on next tick"
                .to_string(),
        });
    }

    // LOADBEARING: realise forces fetch + sig verify; path-equality catches symlink/redirect surprises.
    let store_path = format!("/nix/store/{}", target.closure_hash);
    let realised = match realise(&store_path).await {
        Ok(p) => p,
        Err(RealiseError::SignatureMismatch { stderr_tail }) => {
            tracing::error!(
                target_closure = %target.closure_hash,
                stderr_tail = %stderr_tail,
                "agent: closure signature mismatch - refused by nix substituter trust",
            );
            return Ok(ActivationOutcome::SignatureMismatch {
                closure_hash: target.closure_hash.clone(),
                stderr_tail,
            });
        }
        Err(RealiseError::Other(err)) => {
            tracing::error!(
                target_closure = %target.closure_hash,
                error = %err,
                "agent: realisation failed; not switching",
            );
            return Ok(ActivationOutcome::RealiseFailed {
                reason: err.to_string(),
            });
        }
    };

    if realised != store_path {
        tracing::error!(
            target_closure = %target.closure_hash,
            requested = %store_path,
            realised = %realised,
            "agent: nix-store --realise returned an unexpected path; not switching",
        );
        return Ok(ActivationOutcome::RealiseFailed {
            reason: format!("realised path {realised} does not match requested {store_path}",),
        });
    }

    // LOADBEARING: set profile BEFORE fire - switch-to-configuration {boot,switch,test}
    // reads `/nix/var/nix/profiles/system` to derive the generation number it
    // writes into bootloader entries. The bootloader update itself happens
    // inside the backend's fire_switch (live switch on the happy path,
    // explicit `switch-to-configuration boot` on the defer path).
    let set_status = Command::new("nix-env")
        .arg("--profile")
        .arg("/nix/var/nix/profiles/system")
        .arg("--set")
        .arg(&store_path)
        .status()
        .await
        .with_context(|| "spawn nix-env --set")?;

    if !set_status.success() {
        tracing::error!(
            target_closure = %target.closure_hash,
            exit_code = ?set_status.code(),
            "agent: nix-env --set failed; not running switch-to-configuration",
        );
        return Ok(ActivationOutcome::SwitchFailed {
            phase: "nix-env-set".to_string(),
            exit_code: set_status.code(),
        });
    }

    // LOADBEARING: pre-switch basename feeds flip-to-unexpected detection - abort if read fails.
    let previous_basename = match read_current_system_basename().await {
        Ok(b) => b,
        Err(err) => {
            tracing::error!(
                target_closure = %target.closure_hash,
                error = %err,
                "agent: cannot read /run/current-system pre-switch; aborting activation",
            );
            return Ok(ActivationOutcome::RealiseFailed {
                reason: format!("pre-switch /run/current-system read failed: {err}"),
            });
        }
    };

    if let Some(outcome) = backend.fire_switch(target, &store_path).await? {
        return Ok(outcome);
    }

    // GOTCHA: SIGTERM mid-poll is OK - detached switch unit lands, boot-recovery confirms retroactively.
    let expected = &target.closure_hash;
    match VerifyPoll::new(expected)
        .with_previous(&previous_basename)
        .until_settled()
        .await
    {
        PollOutcome::Settled => {
            // GOTCHA: activation script may re-point profile after our set - self-correct or boot pointer drifts.
            if let Err(err) = self_correct_profile(&store_path).await {
                tracing::warn!(
                    error = %err,
                    "agent: profile self-correction failed (non-fatal); current-system OK so activation continues",
                );
            }
            tracing::info!(
                target_closure = %expected,
                "agent: activation fire-and-forget complete (poll observed expected closure)",
            );
            Ok(ActivationOutcome::FiredAndPolled)
        }
        PollOutcome::Timeout { last_observed } => {
            let exit_code = backend.read_unit_exit_code("nixfleet-switch.service").await;
            tracing::error!(
                target_closure = %expected,
                last_observed = %last_observed,
                exit_code = ?exit_code,
                "agent: switch poll timed out - declaring SwitchFailed",
            );
            Ok(ActivationOutcome::SwitchFailed {
                phase: "switch-poll-timeout".to_string(),
                exit_code,
            })
        }
        PollOutcome::FlippedToUnexpected { observed } => {
            tracing::error!(
                target_closure = %expected,
                actual = %observed,
                previous = %previous_basename,
                "agent: post-switch verify caught flip to unexpected closure - rolling back",
            );
            Ok(ActivationOutcome::VerifyMismatch {
                expected: expected.clone(),
                actual: observed,
            })
        }
    }
}
