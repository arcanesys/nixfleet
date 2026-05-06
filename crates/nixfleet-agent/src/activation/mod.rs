//! Agent-side activation: pre-realise (force fetch + sig verify) → switch via
//! the platform `ActivationBackend` → post-verify `/run/current-system` against
//! the expected closure or rollback. CP-side magic-rollback is independent.

use anyhow::Result;
use nixfleet_proto::agent_wire::EvaluatedTarget;

mod pipeline;
mod profile;
mod realise;
#[path = "rollback.rs"]
mod rollback_mod;
mod types;
mod verify_poll;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod darwin;

pub use pipeline::activate_with;
pub use realise::RealiseError;
pub use rollback_mod::rollback_with;
pub use types::{ActivationBackend, DefaultBackend, DEFAULT_BACKEND};
pub use types::{ActivationOutcome, RollbackOutcome, POLL_BUDGET, POLL_INTERVAL};
#[cfg(target_os = "linux")]
pub use types::LinuxBackend;
#[cfg(target_os = "macos")]
pub use types::DarwinBackend;

/// Single attempt per call; retry is the main poll loop's job (in-call retry
/// would trip the CP confirm deadline since each attempt eats `POLL_BUDGET`).
pub async fn activate(target: &EvaluatedTarget) -> Result<ActivationOutcome> {
    activate_with(&DEFAULT_BACKEND, target).await
}

/// Caller must invoke exactly once per failed activation.
pub async fn rollback() -> Result<RollbackOutcome> {
    rollback_with(&DEFAULT_BACKEND).await
}

pub async fn confirm_target(
    client: &reqwest::Client,
    cp_url: &str,
    hostname: &str,
    target: &EvaluatedTarget,
    rollout: &str,
    wave: u32,
    boot_id: &str,
) -> Result<crate::comms::ConfirmOutcome> {
    use nixfleet_proto::agent_wire::{ConfirmRequest, GenerationRef};

    let req = ConfirmRequest {
        hostname: hostname.to_string(),
        rollout: rollout.to_string(),
        wave,
        generation: GenerationRef {
            closure_hash: target.closure_hash.clone(),
            channel_ref: Some(target.channel_ref.clone()),
            boot_id: boot_id.to_string(),
        },
    };

    // LOADBEARING: refuse to confirm a target the CP didn't tell us how to
    // confirm. Per the wire contract, every confirmable target carries an
    // activate block with a populated confirm_endpoint; absence is treated
    // as "this target is not for confirmation" (e.g. agent-egress exemption
    // dispatches that activate but don't soak through the CP's deadline gate).
    let endpoint = target
        .activate
        .as_ref()
        .map(|a| a.confirm_endpoint.as_str())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "target {} has no activate block; refusing to confirm — \
                 wire contract requires CP to set activate.confirm_endpoint",
                target.closure_hash,
            )
        })?;
    let outcome = crate::comms::confirm(client, cp_url, endpoint, &req).await?;
    match outcome {
        crate::comms::ConfirmOutcome::Acknowledged => {
            tracing::info!(
                target_closure = %target.closure_hash,
                rollout,
                wave,
                "agent: confirm acknowledged (204)",
            );
        }
        crate::comms::ConfirmOutcome::Cancelled => {
            tracing::warn!(
                target_closure = %target.closure_hash,
                rollout,
                "agent: confirm returned 410 — CP says trigger local rollback",
            );
        }
        crate::comms::ConfirmOutcome::Other => {
            tracing::warn!(
                target_closure = %target.closure_hash,
                rollout,
                "agent: confirm returned unexpected status — deadline timer will handle",
            );
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use anyhow::{anyhow, Result};

    use super::types::ActivationBackend;
    use super::types::{ActivationOutcome, RollbackOutcome};
    use super::realise::looks_like_signature_error;
    use super::verify_poll::{PollOutcome, VerifyPoll};
    use super::DEFAULT_BACKEND;

    use nixfleet_proto::agent_wire::EvaluatedTarget;

    fn basename_of(target: &Path) -> Result<String> {
        target
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("no utf-8 basename: {}", target.display()))
    }

    async fn read_current_basename_for_tests() -> Result<String> {
        let target = tokio::fs::read_link("/run/current-system")
            .await
            .map_err(|e| anyhow!("readlink /run/current-system: {e}"))?;
        basename_of(&target)
    }

    #[test]
    fn basename_extracts_from_typical_store_path() {
        let p = PathBuf::from("/nix/store/abc123-nixos-system-test-host-26.05");
        assert_eq!(basename_of(&p).unwrap(), "abc123-nixos-system-test-host-26.05");
    }

    #[tokio::test]
    async fn confirm_target_refuses_without_activate_block() {
        // Wire contract: CP must always set activate.confirm_endpoint for
        // any target the agent should confirm. Absence is treated as "not
        // for confirmation" — the helper errors out before any HTTP call.
        let target = EvaluatedTarget {
            closure_hash: "abc-test".to_string(),
            channel_ref: "stable@deadbeef".to_string(),
            evaluated_at: chrono::Utc::now(),
            rollout_id: "stable@deadbeef".to_string(),
            wave_index: Some(0),
            activate: None,
            signed_at: chrono::Utc::now(),
            freshness_window_secs: 3600,
            compliance_mode: None,
        };
        let client = reqwest::Client::new();
        let err = super::confirm_target(
            &client,
            "https://cp.invalid:0",
            "test-host",
            &target,
            "stable@deadbeef",
            0,
            "boot-id-test",
        )
        .await
        .expect_err("must error when activate block is absent");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("activate")
                && msg.contains("refusing to confirm"),
            "error message must explain the wire-contract violation; got: {msg}",
        );
    }

    #[test]
    fn basename_unchanged_by_trailing_slash() {
        let p = PathBuf::from("/nix/store/abc123-nixos-system-test-host-26.05/");
        assert_eq!(basename_of(&p).unwrap(), "abc123-nixos-system-test-host-26.05");
    }

    #[test]
    fn outcome_kinds_are_distinct() {
        let outcomes = [
            format!("{:?}", ActivationOutcome::FiredAndPolled),
            format!(
                "{:?}",
                ActivationOutcome::RealiseFailed {
                    reason: "x".into()
                }
            ),
            format!(
                "{:?}",
                ActivationOutcome::SwitchFailed {
                    phase: "switch-poll-timeout".into(),
                    exit_code: Some(1),
                }
            ),
            format!(
                "{:?}",
                ActivationOutcome::SignatureMismatch {
                    closure_hash: "h".into(),
                    stderr_tail: "x".into(),
                }
            ),
            format!(
                "{:?}",
                ActivationOutcome::VerifyMismatch {
                    expected: "e".into(),
                    actual: "a".into(),
                }
            ),
        ];
        let unique: std::collections::HashSet<_> = outcomes.iter().collect();
        assert_eq!(unique.len(), outcomes.len(), "outcome variants collide on Debug");
    }

    fn short_poll<'a>(
        expected: &'a str,
        previous: Option<&'a str>,
        budget_ms: u64,
    ) -> VerifyPoll<'a> {
        let mut p = VerifyPoll::new(expected);
        p.previous_basename = previous;
        p.budget = Duration::from_millis(budget_ms);
        p.interval = Duration::from_millis(10);
        p
    }

    #[tokio::test]
    async fn verify_poll_settles_when_match_appears() {
        if !std::path::Path::new("/run/current-system").exists() {
            return;
        }
        let basename = read_current_basename_for_tests().await.unwrap();
        let outcome = short_poll(&basename, None, 100).until_settled().await;
        assert!(
            matches!(outcome, PollOutcome::Settled),
            "poll did not match its own current-system: {outcome:?}",
        );
    }

    #[tokio::test]
    async fn verify_poll_times_out_when_no_match_and_previous_disabled() {
        if !std::path::Path::new("/run/current-system").exists() {
            return;
        }
        let outcome = short_poll("definitely-not-a-real-closure-hash-xyz", None, 50)
            .until_settled()
            .await;
        match outcome {
            PollOutcome::Timeout { last_observed } => {
                assert!(
                    !last_observed.starts_with("<no-reads-completed>"),
                    "expected at least one observation before timeout: {last_observed}",
                );
            }
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn verify_poll_flips_when_observed_is_neither_expected_nor_previous() {
        if !std::path::Path::new("/run/current-system").exists() {
            return;
        }
        let actual = read_current_basename_for_tests().await.unwrap();
        let outcome = short_poll("expected-is-wrong", Some("previous-is-also-wrong"), 100)
            .until_settled()
            .await;
        match outcome {
            PollOutcome::FlippedToUnexpected { observed } => {
                assert_eq!(observed, actual, "observed should be the live basename");
            }
            other => panic!("expected FlippedToUnexpected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn verify_poll_keeps_polling_when_observed_matches_previous() {
        if !std::path::Path::new("/run/current-system").exists() {
            return;
        }
        let actual = read_current_basename_for_tests().await.unwrap();
        let outcome = short_poll("expected-is-wrong", Some(&actual), 50)
            .until_settled()
            .await;
        match outcome {
            PollOutcome::Timeout { last_observed } => {
                assert_eq!(last_observed, actual);
            }
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[test]
    fn switch_failed_phase_strings_are_stable() {
        // LOADBEARING: phase strings are wire contract — rename breaks CP correlation.
        for phase in &[
            "nix-env-set",
            "systemd-run-fire",
            "switch-poll-timeout",
            "switch-poll-mismatch",
            "darwin-activate-spawn",
        ] {
            let outcome = ActivationOutcome::SwitchFailed {
                phase: (*phase).to_string(),
                exit_code: None,
            };
            let _ = format!("{outcome:?}");
        }
    }

    #[tokio::test]
    async fn default_backend_is_switch_in_progress_short_circuits_on_darwin() {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            DEFAULT_BACKEND.is_switch_in_progress(),
        )
        .await;
        assert!(
            result.is_ok(),
            "DEFAULT_BACKEND.is_switch_in_progress should return promptly on a host with no in-flight switch",
        );
        assert!(
            !result.unwrap(),
            "DEFAULT_BACKEND.is_switch_in_progress: no switch in flight on a clean dev host",
        );
    }

    #[test]
    fn darwin_activate_spawn_phase_string_is_stable() {
        let outcome = ActivationOutcome::SwitchFailed {
            phase: "darwin-activate-spawn".to_string(),
            exit_code: None,
        };
        let s = format!("{outcome:?}");
        assert!(s.contains("darwin-activate-spawn"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn read_unit_exit_code_short_circuits_on_darwin() {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            DEFAULT_BACKEND.read_unit_exit_code("definitely-not-a-real-unit.service"),
        )
        .await
        .expect("must return promptly");
        assert!(result.is_none());
    }

    #[test]
    fn detect_signature_error_matches_nix_2_18_phrasing() {
        let s = "error: cannot add path '/nix/store/abc-foo' because \
                 it lacks a valid signature";
        assert!(looks_like_signature_error(s));
    }

    #[test]
    fn detect_signature_error_matches_no_signature_trusted() {
        let s = "error: no signature is trusted by any of these keys: cache.example.com-1";
        assert!(looks_like_signature_error(s));
    }

    #[test]
    fn detect_signature_error_matches_legacy_phrasing() {
        let s = "error: path '/nix/store/abc-foo' is not signed by any of the keys in \
                 trusted-public-keys";
        assert!(looks_like_signature_error(s));
    }

    #[test]
    fn detect_signature_error_matches_no_signatures_matched() {
        let s = "error: no signatures matched any of the configured public keys";
        assert!(looks_like_signature_error(s));
    }

    #[test]
    fn detect_signature_error_matches_signature_mismatch() {
        let s = "warning: signature mismatch for path '/nix/store/abc-foo'";
        assert!(looks_like_signature_error(s));
    }

    #[test]
    fn detect_signature_error_does_not_match_network_failure() {
        let s = "error: unable to download 'https://cache.example.com/nar/abc.nar': \
                 Couldn't connect to server";
        assert!(!looks_like_signature_error(s));
    }

    #[test]
    fn detect_signature_error_does_not_match_missing_path() {
        let s = "error: path '/nix/store/abc-foo' is required, but it has no substitutes \
                 and there is no derivation that produces it";
        assert!(!looks_like_signature_error(s));
    }

    #[test]
    fn detect_signature_error_case_insensitive() {
        let s = "Error: path Lacks A Valid Signature on this host";
        assert!(looks_like_signature_error(s));
    }

    struct FakeBackend {
        switch_in_progress: bool,
        unit_exit_code: Option<i32>,
    }
    impl ActivationBackend for FakeBackend {
        async fn is_switch_in_progress(&self) -> bool {
            self.switch_in_progress
        }
        async fn read_unit_exit_code(&self, _unit_name: &str) -> Option<i32> {
            self.unit_exit_code
        }
        async fn fire_switch(
            &self,
            _target: &EvaluatedTarget,
            _store_path: &str,
        ) -> Result<Option<ActivationOutcome>> {
            unreachable!("fire_switch unused in this test")
        }
        async fn fire_rollback(
            &self,
            _target_basename: &str,
        ) -> Result<Option<RollbackOutcome>> {
            unreachable!("fire_rollback unused in this test")
        }
    }

    #[tokio::test]
    async fn activation_backend_trait_dispatches_to_fake() {
        let fake = FakeBackend {
            switch_in_progress: true,
            unit_exit_code: Some(42),
        };
        assert!(fake.is_switch_in_progress().await);
        assert_eq!(fake.read_unit_exit_code("anything").await, Some(42));

        let fake2 = FakeBackend {
            switch_in_progress: false,
            unit_exit_code: None,
        };
        assert!(!fake2.is_switch_in_progress().await);
        assert!(fake2.read_unit_exit_code("anything").await.is_none());
    }
}
