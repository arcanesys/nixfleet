//! Agent ↔ control-plane wire types.
//!
//! LOADBEARING: additions within a major version MUST be backwards-compatible
//! (older consumers serde-ignore unknown fields). Bump `PROTOCOL_MAJOR_VERSION`
//! for any breaking change — the CP rejects mismatched majors with 426.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// Sent in `X-Nixfleet-Protocol`; CP rejects mismatched majors with 426.
pub const PROTOCOL_MAJOR_VERSION: u32 = 1;

pub const PROTOCOL_VERSION_HEADER: &str = "x-nixfleet-protocol";

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinRequest {
    pub hostname: String,
    pub agent_version: String,

    /// `/run/current-system`.
    pub current_generation: GenerationRef,

    /// `/run/booted-system` when it differs from current.
    #[serde(default)]
    pub pending_generation: Option<PendingGeneration>,

    #[serde(default)]
    pub last_evaluated_target: Option<EvaluatedTarget>,

    #[serde(default)]
    pub last_fetch_outcome: Option<FetchOutcome>,

    /// Surfaces crash-loops that don't show up as offline.
    #[serde(default)]
    pub uptime_secs: Option<u64>,

    /// CP repopulates `last_healthy_since` after rebuild from this; clamped to
    /// `min(now, last_confirmed_at)` so clock skew can't fast-forward soak.
    #[serde(default)]
    pub last_confirmed_at: Option<DateTime<Utc>>,

    /// Base64 ed25519 over JCS(`LastConfirmedAtSignedPayload`) signed with the
    /// host's SSH key. Without it the attested time is silently ignored —
    /// a compromised host can't replay an older confirmation to short-circuit soak.
    #[serde(default)]
    pub attestation_signature: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationRef {
    pub closure_hash: String,
    #[serde(default)]
    pub channel_ref: Option<String>,
    pub boot_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingGeneration {
    pub closure_hash: String,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluatedTarget {
    pub closure_hash: String,
    pub channel_ref: String,
    pub evaluated_at: DateTime<Utc>,
    /// Format: `<channel>@<short-ci-commit-or-closure>`.
    pub rollout_id: String,
    /// 0-based index in `fleet.waves[host.channel]`; `None` for single-host channels.
    #[serde(default)]
    pub wave_index: Option<u32>,
    #[serde(default)]
    pub activate: Option<ActivateBlock>,
    /// `meta.signedAt` of the producing fleet.resolved — relayed for the
    /// agent's defense-in-depth freshness check.
    pub signed_at: DateTime<Utc>,
    pub freshness_window_secs: u32,
    /// `disabled` | `permissive` | `enforce` | `auto`. None → agent auto-detects.
    #[serde(default)]
    pub compliance_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivateBlock {
    /// Seconds before CP triggers magic rollback.
    pub confirm_window_secs: u32,
    /// Required for any target the agent will confirm. The agent refuses to
    /// confirm when no `activate` block is present (treats absence as "not a
    /// confirmable target") and otherwise POSTs strictly to this path. CP
    /// must always set it for confirm-bearing targets — the agent has no
    /// hardcoded fallback. Wire-carried so endpoint moves are CP-driven.
    pub confirm_endpoint: String,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchOutcome {
    pub result: FetchResult,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FetchResult {
    Ok,
    VerifyFailed,
    FetchFailed,
    None,
}

/// CP-driven rollback directive. Idempotent at the protocol level: the
/// agent's rollback is a no-op once it's on the prior gen, so a lost
/// `RollbackTriggered` post retries on the next checkin.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackSignal {
    /// Content-addressed rolloutId of the failed rollout the CP is asking
    /// the agent to revert from.
    pub rollout: String,
    /// Provenance only; the agent rolls to its own boot-loader prior entry.
    pub target_ref: String,
    pub reason: String,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinResponse {
    #[serde(default)]
    pub target: Option<EvaluatedTarget>,
    /// Mutually exclusive with `target` in practice; if both set, the
    /// agent runs rollback synchronously before fetching `target`.
    #[serde(default)]
    pub rollback: Option<RollbackSignal>,
    pub next_checkin_secs: u32,
}

/// Posted exactly once after a new generation has booted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmRequest {
    pub hostname: String,
    /// Format `<channel>@<ref>`.
    pub rollout: String,
    pub wave: u32,
    pub generation: GenerationRef,
}

/// 204 on acceptance, 410 if the rollout was cancelled / wave failed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmResponse {}

/// Out-of-band event report. `rollout = None` for events not tied to one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportRequest {
    pub hostname: String,
    pub agent_version: String,
    pub occurred_at: DateTime<Utc>,
    #[serde(default)]
    pub rollout: Option<String>,
    #[serde(flatten)]
    pub event: ReportEvent,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "details", rename_all = "kebab-case")]
pub enum ReportEvent {
    /// Observability-only pre-fire signal.
    ActivationStarted {
        closure_hash: String,
        channel_ref: String,
    },

    /// Profile flipped via `nix-env --set` but live switch was skipped because
    /// a critical-component swap (dbus impl, systemd, kernel, init) would have
    /// been refused by `nixos-rebuild`'s switchInhibitors check. New generation
    /// activates on next boot. Operator surface: `pending_reboot` in /v1/hosts.
    ActivationDeferred {
        closure_hash: String,
        channel_ref: String,
        /// Component that triggered the inhibitor (e.g. "dbus", "systemd").
        component: String,
    },

    /// Agent gave up retrying a closure after a recent SwitchFailed/VerifyMismatch
    /// (issue #55). Suppression auto-clears when the channel-ref advances to a
    /// different closure_hash. Distinct from `ActivationFailed` (a single failure
    /// observation) — `RolloutQuarantined` signals "agent has decided not to
    /// re-attempt this closure" so the operator can distinguish a transient
    /// hiccup from a permanently-broken release. Unsigned (observability-only;
    /// see `apply_deferred_pending_reboot_transition` precedent: state-driving
    /// effect is internal to CP, no fleet gate reads the signature).
    RolloutQuarantined {
        closure_hash: String,
        channel_ref: String,
        /// Total SwitchFailed/VerifyMismatch observations for this closure_hash.
        failure_count: u32,
        /// Free-form trigger summary (e.g. "switch-poll-timeout exit=2",
        /// "post-switch verify mismatch"). Operator-visible; not parsed.
        reason: String,
    },

    ActivationFailed {
        phase: String,
        #[serde(default)]
        exit_code: Option<i32>,
        #[serde(default)]
        stderr_tail: Option<String>,
        #[serde(default)]
        signature: Option<String>,
    },

    /// `nix-store --realise` failed; agent did not switch.
    RealiseFailed {
        closure_hash: String,
        reason: String,
        #[serde(default)]
        signature: Option<String>,
    },

    /// Post-switch verify caught `/run/current-system` mismatch; rolled back.
    VerifyMismatch {
        expected: String,
        actual: String,
        #[serde(default)]
        signature: Option<String>,
    },

    RollbackTriggered {
        reason: String,
        #[serde(default)]
        signature: Option<String>,
    },

    EnrollmentFailed {
        reason: String,
    },

    RenewalFailed {
        reason: String,
    },

    /// `trust.json` failed to parse or was missing at startup.
    TrustError {
        reason: String,
    },

    /// Substituter rejected closure narinfo signature. Distinct from
    /// `RealiseFailed` for dashboard routing of trust vs transient failures.
    ClosureSignatureMismatch {
        closure_hash: String,
        stderr_tail: String,
        #[serde(default)]
        signature: Option<String>,
    },

    /// Agent refused stale fleet.resolved. CP applies same gate; this fires on
    /// clock-skew or CP gate failure.
    StaleTarget {
        closure_hash: String,
        channel_ref: String,
        signed_at: DateTime<Utc>,
        freshness_window_secs: u32,
        age_secs: i64,
        #[serde(default)]
        signature: Option<String>,
    },

    /// `evidence_snippet` is the probe's `checks` JSON truncated to ~1KB.
    ComplianceFailure {
        control_id: String,
        status: String,
        framework_articles: Vec<String>,
        #[serde(default)]
        evidence_snippet: Option<serde_json::Value>,
        evidence_collected_at: DateTime<Utc>,
        #[serde(default)]
        signature: Option<String>,
    },

    /// Manifest fetch/parse failure; agent refuses to act on the dispatch.
    ManifestMissing {
        rollout_id: String,
        reason: String,
        #[serde(default)]
        signature: Option<String>,
    },

    /// Manifest signature didn't verify against `ciReleaseKey`.
    ManifestVerifyFailed {
        rollout_id: String,
        reason: String,
        #[serde(default)]
        signature: Option<String>,
    },

    /// Manifest verified but content/membership/pinned-bytes check failed.
    ManifestMismatch {
        rollout_id: String,
        reason: String,
        #[serde(default)]
        signature: Option<String>,
    },

    /// Runtime gate couldn't produce a verdict (collector failed/timeout/stale).
    /// Distinct from `ComplianceFailure` — CP treats this as a confirm-blocker.
    RuntimeGateError {
        reason: String,
        #[serde(default)]
        collector_exit_code: Option<i32>,
        #[serde(default)]
        evidence_collected_at: Option<DateTime<Utc>>,
        activation_completed_at: DateTime<Utc>,
        #[serde(default)]
        signature: Option<String>,
    },

    /// Catch-all for events that don't yet have a typed variant.
    Other {
        kind: String,
        #[serde(default)]
        detail: Option<serde_json::Value>,
    },
}

impl ReportEvent {
    /// Wire-side `event` discriminator — matches the serde kebab-case rename.
    /// Adding a variant requires updating this match (compiler-enforced) and
    /// the corresponding wire string in lockstep.
    pub fn discriminator(&self) -> &'static str {
        match self {
            Self::ActivationStarted { .. } => "activation-started",
            Self::ActivationDeferred { .. } => "activation-deferred",
            Self::ActivationFailed { .. } => "activation-failed",
            Self::RolloutQuarantined { .. } => "rollout-quarantined",
            Self::RealiseFailed { .. } => "realise-failed",
            Self::VerifyMismatch { .. } => "verify-mismatch",
            Self::RollbackTriggered { .. } => "rollback-triggered",
            Self::EnrollmentFailed { .. } => "enrollment-failed",
            Self::RenewalFailed { .. } => "renewal-failed",
            Self::TrustError { .. } => "trust-error",
            Self::ClosureSignatureMismatch { .. } => "closure-signature-mismatch",
            Self::StaleTarget { .. } => "stale-target",
            Self::ComplianceFailure { .. } => "compliance-failure",
            Self::ManifestMissing { .. } => "manifest-missing",
            Self::ManifestVerifyFailed { .. } => "manifest-verify-failed",
            Self::ManifestMismatch { .. } => "manifest-mismatch",
            Self::RuntimeGateError { .. } => "runtime-gate-error",
            Self::Other { .. } => "other",
        }
    }
}

#[cfg(test)]
mod report_event_discriminator_tests {
    use super::*;

    /// LOADBEARING: discriminator() must match the wire-serialized "event" tag
    /// exactly, since the CP indexes events by the string. Round-trip a value
    /// of every variant through serde and compare against the hand-written
    /// match — if a variant is renamed at the serde layer this test catches it.
    #[test]
    fn discriminator_matches_serde_event_tag() {
        let now = chrono::Utc::now();
        let cases: Vec<ReportEvent> = vec![
            ReportEvent::ActivationStarted {
                closure_hash: "x".into(),
                channel_ref: "y".into(),
            },
            ReportEvent::ActivationDeferred {
                closure_hash: "x".into(),
                channel_ref: "y".into(),
                component: "dbus".into(),
            },
            ReportEvent::RolloutQuarantined {
                closure_hash: "x".into(),
                channel_ref: "y".into(),
                failure_count: 2,
                reason: "switch-poll-timeout exit=2".into(),
            },
            ReportEvent::ActivationFailed {
                phase: "x".into(),
                exit_code: None,
                stderr_tail: None,
                signature: None,
            },
            ReportEvent::RealiseFailed {
                closure_hash: "x".into(),
                reason: "y".into(),
                signature: None,
            },
            ReportEvent::VerifyMismatch {
                expected: "x".into(),
                actual: "y".into(),
                signature: None,
            },
            ReportEvent::RollbackTriggered {
                reason: "x".into(),
                signature: None,
            },
            ReportEvent::EnrollmentFailed {
                reason: "x".into(),
            },
            ReportEvent::RenewalFailed {
                reason: "x".into(),
            },
            ReportEvent::TrustError {
                reason: "x".into(),
            },
            ReportEvent::ClosureSignatureMismatch {
                closure_hash: "x".into(),
                stderr_tail: "y".into(),
                signature: None,
            },
            ReportEvent::StaleTarget {
                closure_hash: "x".into(),
                channel_ref: "y".into(),
                signed_at: now,
                freshness_window_secs: 60,
                age_secs: 0,
                signature: None,
            },
            ReportEvent::ComplianceFailure {
                control_id: "x".into(),
                status: "fail".into(),
                framework_articles: vec![],
                evidence_snippet: None,
                evidence_collected_at: now,
                signature: None,
            },
            ReportEvent::ManifestMissing {
                rollout_id: "x".into(),
                reason: "y".into(),
                signature: None,
            },
            ReportEvent::ManifestVerifyFailed {
                rollout_id: "x".into(),
                reason: "y".into(),
                signature: None,
            },
            ReportEvent::ManifestMismatch {
                rollout_id: "x".into(),
                reason: "y".into(),
                signature: None,
            },
            ReportEvent::RuntimeGateError {
                reason: "x".into(),
                collector_exit_code: None,
                evidence_collected_at: None,
                activation_completed_at: now,
                signature: None,
            },
            ReportEvent::Other {
                kind: "x".into(),
                detail: None,
            },
        ];
        for ev in &cases {
            let wire = serde_json::to_value(ev).unwrap();
            let tag = wire
                .get("event")
                .and_then(|v| v.as_str())
                .expect("event tag missing");
            assert_eq!(
                tag,
                ev.discriminator(),
                "wire tag must match discriminator() for {ev:?}",
            );
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportResponse {
    pub event_id: String,
}
