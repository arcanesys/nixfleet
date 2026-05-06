//! Rollback pipeline: nix-env --rollback → discover target → fire → poll.
//!
//! FOOTGUN: bypasses `nixos-rebuild --rollback` because `nixos-rebuild-ng`
//! evaluates `<nixpkgs/nixos>` even on rollback, which fails in the agent's
//! NIX_PATH-less sandbox.

use anyhow::{Context, Result};
use tokio::process::Command;

use super::types::ActivationBackend;
use super::types::RollbackOutcome;
use super::profile::resolve_profile_target;
use super::verify_poll::{PollOutcome, VerifyPoll};

pub async fn rollback_with<B: ActivationBackend>(backend: &B) -> Result<RollbackOutcome> {
    tracing::warn!("agent: triggering local rollback (fire-and-forget via systemd-run)");

    let env_status = Command::new("nix-env")
        .arg("--profile")
        .arg("/nix/var/nix/profiles/system")
        .arg("--rollback")
        .status()
        .await
        .with_context(|| "spawn nix-env --rollback")?;
    if !env_status.success() {
        tracing::error!(
            exit_code = ?env_status.code(),
            "agent: nix-env --rollback failed; cannot proceed",
        );
        return Ok(RollbackOutcome::Failed {
            phase: "nix-env-rollback".to_string(),
            exit_code: env_status.code(),
        });
    }

    // Discover the rolled-back target so we poll for it specifically.
    let target_basename = match resolve_profile_target() {
        Ok(b) => b,
        Err(err) => {
            tracing::error!(
                error = %err,
                "agent: cannot resolve rolled-back profile target; aborting rollback",
            );
            return Ok(RollbackOutcome::Failed {
                phase: "discover-target".to_string(),
                exit_code: None,
            });
        }
    };
    tracing::info!(
        target_basename = %target_basename,
        "agent: rollback target discovered; firing detached switch",
    );

    if let Some(failure) = backend.fire_rollback(&target_basename).await? {
        return Ok(failure);
    }

    // `previous_basename` stays None: the failed gen we're abandoning is not
    // a meaningful pre-state, so any non-match collapses into the timeout branch.
    match VerifyPoll::new(&target_basename).until_settled().await {
        PollOutcome::Settled => {
            tracing::info!(
                target = %target_basename,
                "agent: rollback fire-and-forget complete",
            );
            Ok(RollbackOutcome::FiredAndPolled)
        }
        PollOutcome::Timeout { last_observed } => {
            let exit_code = backend.read_unit_exit_code("nixfleet-rollback.service").await;
            tracing::error!(
                target = %target_basename,
                last_observed = %last_observed,
                exit_code = ?exit_code,
                "agent: rollback poll timed out",
            );
            Ok(RollbackOutcome::Failed {
                phase: "rollback-poll-timeout".to_string(),
                exit_code,
            })
        }
        PollOutcome::FlippedToUnexpected { .. } => {
            unreachable!(
                "FlippedToUnexpected requires Some(previous_basename); rollback leaves it None"
            )
        }
    }
}
