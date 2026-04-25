//! Shared signing-payload shapes for host probe-output evidence. Adding a
//! field invalidates existing signatures - bump signing version.

use chrono::{DateTime, Utc};
use serde::Serialize;

/// `evidence_snippet_sha256` hashes the JCS bytes of the snippet to keep the
/// signed payload bounded.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComplianceFailureSignedPayload<'a> {
    pub hostname: &'a str,
    pub rollout: Option<&'a str>,
    pub control_id: &'a str,
    pub status: &'a str,
    pub framework_articles: &'a [String],
    pub evidence_collected_at: DateTime<Utc>,
    pub evidence_snippet_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeGateErrorSignedPayload<'a> {
    pub hostname: &'a str,
    pub rollout: Option<&'a str>,
    pub reason: &'a str,
    pub collector_exit_code: Option<i32>,
    pub evidence_collected_at: Option<DateTime<Utc>>,
    pub activation_completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationFailedSignedPayload<'a> {
    pub hostname: &'a str,
    pub rollout: Option<&'a str>,
    pub phase: &'a str,
    pub exit_code: Option<i32>,
    /// SHA-256 of the JCS bytes of `stderr_tail`.
    pub stderr_tail_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackTriggeredSignedPayload<'a> {
    pub hostname: &'a str,
    pub rollout: Option<&'a str>,
    pub reason: &'a str,
}

/// Soak-state attestation, bound to (hostname, rollout) so a stale signature
/// can't replay across rollouts. Without this signature CP cannot trust the
/// agent's claimed confirmation time (replay would short-circuit the soak gate).
/// Verified against `hosts.<hostname>.pubkey` from fleet.resolved.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastConfirmedAtSignedPayload<'a> {
    pub hostname: &'a str,
    pub rollout_id: &'a str,
    pub last_confirmed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealiseFailedSignedPayload<'a> {
    pub hostname: &'a str,
    pub rollout: Option<&'a str>,
    pub closure_hash: &'a str,
    pub reason: &'a str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyMismatchSignedPayload<'a> {
    pub hostname: &'a str,
    pub rollout: Option<&'a str>,
    pub expected: &'a str,
    pub actual: &'a str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosureSignatureMismatchSignedPayload<'a> {
    pub hostname: &'a str,
    pub rollout: Option<&'a str>,
    pub closure_hash: &'a str,
    /// SHA-256 of the JCS bytes of `stderr_tail`.
    pub stderr_tail_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaleTargetSignedPayload<'a> {
    pub hostname: &'a str,
    pub rollout: Option<&'a str>,
    pub closure_hash: &'a str,
    pub channel_ref: &'a str,
    pub signed_at: DateTime<Utc>,
    pub freshness_window_secs: u32,
    pub age_secs: i64,
}

/// Agent could not load + parse the advertised rollout manifest.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestMissingSignedPayload<'a> {
    pub hostname: &'a str,
    pub rollout: Option<&'a str>,
    pub rollout_id: &'a str,
    pub reason: &'a str,
}

/// Manifest signature didn't verify against trust roots.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestVerifyFailedSignedPayload<'a> {
    pub hostname: &'a str,
    pub rollout: Option<&'a str>,
    pub rollout_id: &'a str,
    pub reason: &'a str,
}

/// Manifest signed but agent's content-bound checks failed (hash, host_set
/// membership, or pinned-bytes drift).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestMismatchSignedPayload<'a> {
    pub hostname: &'a str,
    pub rollout: Option<&'a str>,
    pub rollout_id: &'a str,
    pub reason: &'a str,
}
