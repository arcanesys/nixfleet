//! Dispatch path: `process_dispatch_target` + per-outcome free-function handlers.
//! Side-effects route through `&impl Reporter` for unit-testability.

mod activate;
pub(crate) mod compliance;
mod confirm;
mod deferred;
mod manifest_error;
mod quarantined;
mod realise_failed;
mod rollback;
mod verify_mismatch;

pub(crate) use activate::process_dispatch_target;
pub(crate) use rollback::handle_cp_rollback_signal;

use std::sync::Arc;

use nixfleet_proto::agent_wire::EvaluatedTarget;
use serde::Serialize;

use nixfleet_agent::comms::Reporter;
use nixfleet_agent::evidence_signer::{try_sign, EvidenceSigner};

use crate::Args;

/// Shared dispatch context. Handlers are free functions in the sibling
/// modules — telemetry-only, never propagate errors.
pub(crate) struct DispatchCtx<'a, R: Reporter> {
    pub target: &'a EvaluatedTarget,
    pub reporter: &'a R,
    pub args: &'a Args,
    pub evidence_signer: &'a Arc<Option<EvidenceSigner>>,
}

impl<R: Reporter> DispatchCtx<'_, R> {
    /// JCS-sign `payload` with the agent's evidence key, if configured.
    pub(super) fn try_sign<T: Serialize>(&self, payload: &T) -> Option<String> {
        self.evidence_signer
            .as_ref()
            .as_ref()
            .and_then(|s| try_sign(s, payload))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use nixfleet_agent::comms::Reporter;
    use nixfleet_agent::evidence_signer::EvidenceSigner;
    use nixfleet_proto::agent_wire::{EvaluatedTarget, ReportEvent};

    use super::DispatchCtx;
    use super::deferred::handle_deferred_pending_reboot;
    use super::quarantined::{evaluate as evaluate_quarantine, post_quarantine_event, QuarantineDecision};
    use super::realise_failed::{handle_closure_signature_mismatch, handle_realise_failed};
    use crate::Args;

    #[derive(Default)]
    struct FakeReporter {
        calls: Mutex<Vec<(Option<String>, ReportEvent)>>,
    }
    impl FakeReporter {
        fn new() -> Self {
            Self::default()
        }
        fn calls(&self) -> Vec<(Option<String>, ReportEvent)> {
            self.calls.lock().unwrap().clone()
        }
    }
    impl Reporter for FakeReporter {
        async fn post_report(&self, rollout: Option<&str>, event: ReportEvent) {
            self.calls
                .lock()
                .unwrap()
                .push((rollout.map(String::from), event));
        }
    }

    fn sample_target() -> EvaluatedTarget {
        EvaluatedTarget {
            closure_hash: "abc123-test".to_string(),
            channel_ref: "stable@feedface".to_string(),
            evaluated_at: chrono::Utc::now(),
            rollout_id: "stable@feedface".to_string(),
            wave_index: None,
            activate: None,
            signed_at: chrono::Utc::now(),
            freshness_window_secs: 3600,
            compliance_mode: None,
        }
    }

    fn sample_args() -> Args {
        Args {
            control_plane_url: "https://cp.test".to_string(),
            machine_id: "test-host".to_string(),
            poll_interval: 60,
            trust_file: PathBuf::from("/dev/null"),
            ca_cert: None,
            client_cert: None,
            client_key: None,
            bootstrap_token_file: None,
            state_dir: PathBuf::from("/tmp/nixfleet-test"),
            compliance_gate_mode: None,
            ssh_host_key_file: PathBuf::from("/dev/null"),
            health_checks_config: None,
        }
    }

    fn ctx<'a, R: Reporter>(
        target: &'a EvaluatedTarget,
        reporter: &'a R,
        args: &'a Args,
        signer: &'a Arc<Option<EvidenceSigner>>,
    ) -> DispatchCtx<'a, R> {
        DispatchCtx {
            target,
            reporter,
            args,
            evidence_signer: signer,
        }
    }

    #[tokio::test]
    async fn closure_signature_mismatch_handler_posts_signed_event_and_does_not_attempt_rollback() {
        let fake = FakeReporter::new();
        let target = sample_target();
        let args = sample_args();
        let signer: Arc<Option<EvidenceSigner>> = Arc::new(None);

        handle_closure_signature_mismatch(
            &ctx(&target, &fake, &args, &signer),
            "abc123-bad-sig".to_string(),
            "error: lacks a valid signature".to_string(),
        )
        .await;

        let calls = fake.calls();
        assert_eq!(calls.len(), 1, "expected exactly one post; got {:?}", calls);
        let (rollout, event) = &calls[0];
        assert_eq!(rollout.as_deref(), Some("stable@feedface"));
        match event {
            ReportEvent::ClosureSignatureMismatch {
                closure_hash,
                stderr_tail,
                signature,
            } => {
                assert_eq!(closure_hash, "abc123-bad-sig");
                assert_eq!(stderr_tail, "error: lacks a valid signature");
                assert!(
                    signature.is_none(),
                    "no evidence_signer wired → signature must be None",
                );
            }
            other => panic!("expected ClosureSignatureMismatch, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn realise_failed_handler_posts_one_event_no_rollback() {
        let fake = FakeReporter::new();
        let target = sample_target();
        let args = sample_args();
        let signer: Arc<Option<EvidenceSigner>> = Arc::new(None);

        handle_realise_failed(
            &ctx(&target, &fake, &args, &signer),
            "network unreachable".to_string(),
        )
        .await;

        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        match &calls[0].1 {
            ReportEvent::RealiseFailed {
                closure_hash,
                reason,
                ..
            } => {
                assert_eq!(closure_hash, "abc123-test");
                assert_eq!(reason, "network unreachable");
            }
            other => panic!("expected RealiseFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn quarantine_evaluate_suppresses_when_recent_failure_matches_target_closure() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        // Seed a recent failure record for the target's closure_hash.
        nixfleet_agent::checkin_state::record_switch_failure(
            dir.path(),
            "abc123-test",
            "stable@feedface",
            "phase=switch-poll-timeout",
            now,
        )
        .unwrap();
        let target = sample_target();
        match evaluate_quarantine(dir.path(), &target, now + chrono::Duration::seconds(60)) {
            QuarantineDecision::Suppress(rec) => {
                assert_eq!(rec.closure_hash, "abc123-test");
                assert_eq!(rec.failure_count, 1);
            }
            QuarantineDecision::Proceed => {
                panic!("expected Suppress for matching closure within window");
            }
        }
    }

    #[tokio::test]
    async fn quarantine_evaluate_proceeds_when_target_closure_differs() {
        // The whole correctness guarantee for #55's auto-clearing: if the
        // channel-ref advances to a fresher closure_hash, the suppression
        // bypasses naturally without needing an explicit clear.
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        nixfleet_agent::checkin_state::record_switch_failure(
            dir.path(),
            "stale-hash",
            "stable@old",
            "phase=switch",
            now,
        )
        .unwrap();
        let target = sample_target(); // closure_hash = abc123-test
        assert!(matches!(
            evaluate_quarantine(dir.path(), &target, now),
            QuarantineDecision::Proceed,
        ));
    }

    #[tokio::test]
    async fn quarantine_evaluate_proceeds_after_window_expires() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        let target = sample_target();
        nixfleet_agent::checkin_state::record_switch_failure(
            dir.path(),
            &target.closure_hash,
            &target.channel_ref,
            "phase=switch",
            now,
        )
        .unwrap();
        // 25 hours later — outside the 24h quarantine window.
        let later = now + chrono::Duration::hours(25);
        assert!(matches!(
            evaluate_quarantine(dir.path(), &target, later),
            QuarantineDecision::Proceed,
        ));
    }

    #[tokio::test]
    async fn post_quarantine_event_throttles_repeat_posts_within_window() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        let target = sample_target();
        nixfleet_agent::checkin_state::record_switch_failure(
            dir.path(),
            &target.closure_hash,
            &target.channel_ref,
            "phase=switch",
            now,
        )
        .unwrap();

        let fake = FakeReporter::new();
        let mut args = sample_args();
        args.state_dir = dir.path().to_path_buf();
        let signer: Arc<Option<EvidenceSigner>> = Arc::new(None);

        let record_first =
            match evaluate_quarantine(&args.state_dir, &target, now + chrono::Duration::seconds(60))
            {
                QuarantineDecision::Suppress(r) => r,
                _ => panic!("expected Suppress"),
            };
        post_quarantine_event(
            &ctx(&target, &fake, &args, &signer),
            record_first,
            now + chrono::Duration::seconds(60),
        )
        .await;
        assert_eq!(fake.calls().len(), 1, "first suppression posts");

        // 5 minutes later — inside the 1h throttle window — should NOT re-post.
        let record_second =
            match evaluate_quarantine(&args.state_dir, &target, now + chrono::Duration::seconds(360))
            {
                QuarantineDecision::Suppress(r) => r,
                _ => panic!("expected Suppress"),
            };
        post_quarantine_event(
            &ctx(&target, &fake, &args, &signer),
            record_second,
            now + chrono::Duration::seconds(360),
        )
        .await;
        assert_eq!(fake.calls().len(), 1, "throttled within 1h window");

        // 70 minutes after first post — past the throttle — should re-post.
        let record_third =
            match evaluate_quarantine(&args.state_dir, &target, now + chrono::Duration::seconds(60 + 4200))
            {
                QuarantineDecision::Suppress(r) => r,
                _ => panic!("expected Suppress"),
            };
        post_quarantine_event(
            &ctx(&target, &fake, &args, &signer),
            record_third,
            now + chrono::Duration::seconds(60 + 4200),
        )
        .await;
        assert_eq!(fake.calls().len(), 2, "throttle window elapsed → re-posts");

        // Each post must be ClosureQuarantined with the right shape.
        for (rollout, ev) in fake.calls() {
            assert_eq!(rollout.as_deref(), Some("stable@feedface"));
            match ev {
                ReportEvent::ClosureQuarantined {
                    closure_hash,
                    channel_ref,
                    failure_count,
                    reason,
                } => {
                    assert_eq!(closure_hash, "abc123-test");
                    assert_eq!(channel_ref, "stable@feedface");
                    assert_eq!(failure_count, 1);
                    assert!(reason.contains("switch"));
                }
                other => panic!("expected ClosureQuarantined, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn deferred_pending_reboot_handler_posts_activation_deferred_event() {
        let fake = FakeReporter::new();
        let target = sample_target();
        let args = sample_args();
        let signer: Arc<Option<EvidenceSigner>> = Arc::new(None);

        handle_deferred_pending_reboot(
            &ctx(&target, &fake, &args, &signer),
            "dbus".to_string(),
        )
        .await;

        let calls = fake.calls();
        assert_eq!(calls.len(), 1, "expected exactly one post; got {:?}", calls);
        let (rollout, event) = &calls[0];
        assert_eq!(rollout.as_deref(), Some("stable@feedface"));
        match event {
            ReportEvent::ActivationDeferred {
                closure_hash,
                channel_ref,
                component,
            } => {
                assert_eq!(closure_hash, "abc123-test");
                assert_eq!(channel_ref, "stable@feedface");
                assert_eq!(component, "dbus");
            }
            other => panic!("expected ActivationDeferred, got {other:?}"),
        }
    }
}
