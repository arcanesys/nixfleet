//! Boot-time recovery: a post-self-switch agent reads `last_dispatched` to
//! retroactively confirm the in-flight target before its deadline expires.

use std::path::Path;
use std::sync::Arc;

use nixfleet_proto::agent_wire::EvaluatedTarget;

use crate::comms::Reporter;
use crate::evidence_signer::EvidenceSigner;
use crate::{activation, checkin_state, comms};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    NoRecord,
    NoCurrent,
    StaleClearedMismatch,
    /// Enforce-mode runtime gate fired rollback; confirm intentionally skipped.
    GateBlockedConfirm,
    PostedConfirm {
        confirm_outcome: comms::ConfirmOutcome,
    },
    PostedConfirmFailed {
        error: String,
    },
}

/// Inputs the gate needs that the bare confirm path doesn't.
pub struct GateInputs<'a, R: Reporter> {
    pub reporter: &'a R,
    pub evidence_signer: &'a Arc<Option<EvidenceSigner>>,
    pub cli_default_mode: Option<&'a str>,
}

/// Best-effort: failures are logged, never propagated; main poll re-converges.
pub async fn run_boot_recovery<R: Reporter>(
    client: &reqwest::Client,
    state_dir: &Path,
    cp_url: &str,
    hostname: &str,
    current_closure: Option<String>,
    gate: GateInputs<'_, R>,
) -> anyhow::Result<()> {
    let action = decide_and_run(client, state_dir, cp_url, hostname, current_closure, gate).await;
    match &action {
        RecoveryAction::NoRecord => {
            tracing::debug!("boot-recovery: no last_dispatched record (steady-state)");
        }
        RecoveryAction::NoCurrent => {
            tracing::warn!("boot-recovery: skipped - could not read current closure");
        }
        RecoveryAction::StaleClearedMismatch => {
            tracing::info!(
                "boot-recovery: cleared stale dispatch record (current/dispatched mismatch)"
            );
        }
        RecoveryAction::GateBlockedConfirm => {
            tracing::error!("boot-recovery: enforce-mode gate fired rollback; confirm skipped");
        }
        RecoveryAction::PostedConfirm { confirm_outcome } => {
            tracing::info!(
                outcome = ?confirm_outcome,
                "boot-recovery: retroactive confirm posted",
            );
        }
        RecoveryAction::PostedConfirmFailed { error } => {
            tracing::warn!(
                error = %error,
                "boot-recovery: retroactive confirm POST failed; record retained",
            );
        }
    }
    Ok(())
}

async fn decide_and_run<R: Reporter>(
    client: &reqwest::Client,
    state_dir: &Path,
    cp_url: &str,
    hostname: &str,
    current_closure: Option<String>,
    gate: GateInputs<'_, R>,
) -> RecoveryAction {
    let dispatched = match checkin_state::read_last_dispatched(state_dir) {
        Ok(Some(rec)) => rec,
        Ok(None) => return RecoveryAction::NoRecord,
        Err(err) => {
            tracing::warn!(
                error = %err,
                state_dir = %state_dir.display(),
                "boot-recovery: read_last_dispatched failed; treating as absent",
            );
            return RecoveryAction::NoRecord;
        }
    };

    let current = match current_closure {
        Some(c) => c,
        None => return RecoveryAction::NoCurrent,
    };

    if current != dispatched.closure_hash {
        let _ = checkin_state::clear_last_dispatched(state_dir);
        return RecoveryAction::StaleClearedMismatch;
    }

    // Gate runs against the now-active closure with `now` as activation_completed_at;
    // the collector trigger inside `run_runtime_gate` writes fresh evidence so the
    // freshness slack is satisfied. Enforce-mode failures roll back + skip confirm.
    let activation_completed_at = chrono::Utc::now();
    let resolved_mode = crate::compliance::resolve_runtime_gate_mode(
        dispatched.compliance_mode.as_deref(),
        gate.cli_default_mode,
    )
    .await;
    let gate_outcome = crate::compliance::run_runtime_gate(
        activation_completed_at,
        &crate::compliance::default_evidence_path(),
        resolved_mode,
    )
    .await;
    let gate_blocks = crate::compliance::apply_gate_outcome(
        &gate_outcome,
        resolved_mode,
        hostname,
        &dispatched.channel_ref,
        gate.reporter,
        gate.evidence_signer,
        activation_completed_at,
    )
    .await;
    if gate_blocks {
        // LOADBEARING: clearing the record on rollback would mask the failure
        // from a subsequent reboot's recovery; keep it for next-boot retry.
        return RecoveryAction::GateBlockedConfirm;
    }

    let boot_id = crate::host_facts::boot_id().unwrap_or_else(|_| "unknown".to_string());
    // confirm_target does not re-run the freshness gate; signed_at /
    // freshness_window_secs are present only to satisfy the schema. The
    // ActivateBlock.confirm_endpoint is the only field this path actually
    // reads downstream.
    let synthetic_target = EvaluatedTarget {
        closure_hash: dispatched.closure_hash.clone(),
        channel_ref: dispatched.channel_ref.clone(),
        evaluated_at: dispatched.dispatched_at,
        rollout_id: dispatched.rollout_id.clone(),
        wave_index: None,
        activate: Some(nixfleet_proto::agent_wire::ActivateBlock {
            // confirm_window_secs is informational on the agent side and
            // unused post-activation; CP-issued deadline lives in CP state.
            confirm_window_secs: 0,
            confirm_endpoint: dispatched.confirm_endpoint.clone(),
        }),
        signed_at: dispatched.dispatched_at,
        freshness_window_secs: 0,
        compliance_mode: None,
    };

    match activation::confirm_target(
        client,
        cp_url,
        hostname,
        &synthetic_target,
        &dispatched.channel_ref,
        /* wave */ 0,
        &boot_id,
    )
    .await
    {
        Ok(outcome) => {
            match outcome {
                comms::ConfirmOutcome::Acknowledged => {
                    checkin_state::record_confirm_success(
                        state_dir,
                        &synthetic_target,
                        chrono::Utc::now(),
                    );
                }
                comms::ConfirmOutcome::Cancelled => {
                    // LOADBEARING: rollback failure must NOT clear last_dispatched (clearing splits brain).
                    // GOTCHA: rollback() returns Ok(Failed) for in-band failure - inspect outcome, not just Result.
                    match activation::rollback().await {
                        Ok(outcome) if outcome.success() => {
                            let _ = checkin_state::clear_last_dispatched(state_dir);
                        }
                        Ok(outcome) => {
                            tracing::error!(
                                phase = ?outcome.phase(),
                                exit_code = ?outcome.exit_code(),
                                "boot-recovery: rollback FAILED - leaving last_dispatched in place for next-boot retry",
                            );
                        }
                        Err(err) => {
                            tracing::error!(
                                error = %err,
                                "boot-recovery: rollback errored - leaving last_dispatched in place for next-boot retry",
                            );
                        }
                    }
                }
                comms::ConfirmOutcome::Other => {}
            }
            RecoveryAction::PostedConfirm {
                confirm_outcome: outcome,
            }
        }
        Err(err) => RecoveryAction::PostedConfirmFailed {
            error: err.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkin_state::LastDispatchRecord;
    use crate::evidence_signer::EvidenceSigner;
    use chrono::Utc;
    use nixfleet_proto::agent_wire::ReportEvent;
    use std::sync::Mutex;
    use tempfile::TempDir;

    fn dummy_client() -> reqwest::Client {
        reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap()
    }

    fn sample_record(closure: &str) -> LastDispatchRecord {
        LastDispatchRecord {
            closure_hash: closure.to_string(),
            channel_ref: "stable@deadbeef".to_string(),
            rollout_id: "stable@deadbeef".to_string(),
            compliance_mode: None,
            confirm_endpoint: "/v1/agent/confirm".to_string(),
            dispatched_at: Utc::now(),
        }
    }

    #[derive(Default)]
    struct NoopReporter {
        calls: Mutex<Vec<(Option<String>, ReportEvent)>>,
    }
    impl Reporter for NoopReporter {
        async fn post_report(&self, rollout: Option<&str>, event: ReportEvent) {
            self.calls
                .lock()
                .unwrap()
                .push((rollout.map(String::from), event));
        }
    }

    fn no_signer() -> Arc<Option<EvidenceSigner>> {
        Arc::new(None)
    }

    fn gate_inputs<'a>(
        reporter: &'a NoopReporter,
        signer: &'a Arc<Option<EvidenceSigner>>,
    ) -> GateInputs<'a, NoopReporter> {
        GateInputs {
            reporter,
            evidence_signer: signer,
            cli_default_mode: Some("disabled"),
        }
    }

    #[tokio::test]
    async fn no_record_when_state_dir_empty() {
        let dir = TempDir::new().unwrap();
        let reporter = NoopReporter::default();
        let signer = no_signer();
        let action = decide_and_run(
            &dummy_client(),
            dir.path(),
            "https://cp:0",
            "test-host",
            Some("any-closure".to_string()),
            gate_inputs(&reporter, &signer),
        )
        .await;
        assert_eq!(action, RecoveryAction::NoRecord);
    }

    #[tokio::test]
    async fn no_current_when_current_closure_missing() {
        let dir = TempDir::new().unwrap();
        checkin_state::write_last_dispatched(dir.path(), &sample_record("some-closure")).unwrap();
        let reporter = NoopReporter::default();
        let signer = no_signer();
        let action = decide_and_run(
            &dummy_client(),
            dir.path(),
            "https://cp:0",
            "test-host",
            None,
            gate_inputs(&reporter, &signer),
        )
        .await;
        assert_eq!(action, RecoveryAction::NoCurrent);
        assert!(
            checkin_state::read_last_dispatched(dir.path())
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn mismatch_clears_stale_record() {
        let dir = TempDir::new().unwrap();
        checkin_state::write_last_dispatched(dir.path(), &sample_record("dispatched-closure"))
            .unwrap();
        let reporter = NoopReporter::default();
        let signer = no_signer();
        let action = decide_and_run(
            &dummy_client(),
            dir.path(),
            "https://cp:0",
            "test-host",
            Some("different-closure".to_string()),
            gate_inputs(&reporter, &signer),
        )
        .await;
        assert_eq!(action, RecoveryAction::StaleClearedMismatch);
        assert!(
            checkin_state::read_last_dispatched(dir.path())
                .unwrap()
                .is_none(),
            "stale record must be cleared on mismatch",
        );
    }

    #[tokio::test]
    async fn match_attempts_post_and_records_failure_on_unreachable_cp() {
        let dir = TempDir::new().unwrap();
        checkin_state::write_last_dispatched(dir.path(), &sample_record("matching-closure"))
            .unwrap();
        let reporter = NoopReporter::default();
        let signer = no_signer();
        let action = decide_and_run(
            &dummy_client(),
            dir.path(),
            "https://127.0.0.1:1/",
            "test-host",
            Some("matching-closure".to_string()),
            gate_inputs(&reporter, &signer),
        )
        .await;
        match action {
            RecoveryAction::PostedConfirmFailed { error } => {
                assert!(!error.is_empty(), "transport error should carry a message");
            }
            other => panic!("expected PostedConfirmFailed, got {other:?}"),
        }
        assert!(
            checkin_state::read_last_dispatched(dir.path())
                .unwrap()
                .is_some(),
            "unfailed POST must leave the record for the next checkin to retry",
        );
    }
}
