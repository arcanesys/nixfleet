//! Runtime compliance gate: trigger collector + verify fresh evidence
//! before confirm.
//!
//! LOADBEARING: evidence freshness is gated against `activation_completed_at`,
//! NOT wall-clock - without this, a rollout could promote on stale PASS
//! evidence collected before the closure switched.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

pub const DEFAULT_EVIDENCE_PATH: &str = "/var/lib/nixfleet-compliance/evidence.json";

pub const COLLECTOR_UNIT: &str = "compliance-evidence-collector.service";

pub const COLLECTOR_TIMEOUT: Duration = Duration::from_secs(120);

/// Slack for collector-vs-activation timestamps; absorbs runtime noise.
pub const TIMESTAMP_SLACK_SECS: i64 = 60;

pub use nixfleet_proto::compliance::GateMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceEvidence {
    pub host: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub controls: Vec<ControlEvidence>,
    #[serde(default)]
    pub overall: String,
}

/// `framework_articles` is `{nis2: [...], iso27001: [...]}` on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlEvidence {
    pub control: String,
    pub status: String,
    #[serde(default, rename = "framework_articles")]
    pub framework_articles: serde_json::Value,
    #[serde(default)]
    pub checks: serde_json::Value,
}

#[derive(Debug, Clone)]
pub enum GateOutcome {
    Pass {
        evidence: ComplianceEvidence,
    },
    Failures {
        evidence: ComplianceEvidence,
        failures: Vec<ControlEvidence>,
    },
    Skipped {
        reason: String,
    },
    GateError {
        reason: String,
        collector_exit_code: Option<i32>,
        evidence_collected_at: Option<DateTime<Utc>>,
    },
}

/// `None` means auto: Permissive if collector present, Disabled if not.
pub async fn resolve_mode(input: Option<GateMode>) -> GateMode {
    let collector_present = collector_unit_present().await;
    match input {
        Some(GateMode::Disabled) => GateMode::Disabled,
        Some(m @ (GateMode::Permissive | GateMode::Enforce)) if collector_present => m,
        Some(explicit) => {
            tracing::warn!(
                ?explicit,
                "compliance gate configured to enforce/permissive but \
                 {} not present - skipping. Either deploy \
                 nixfleet-compliance or set complianceGate.mode = \"disabled\".",
                COLLECTOR_UNIT
            );
            GateMode::Disabled
        }
        None => {
            if collector_present {
                GateMode::Permissive
            } else {
                GateMode::Disabled
            }
        }
    }
}

/// `Disabled` short-circuits to `Skipped`; caller decides whether `GateError`
/// blocks confirm (the gate body itself does not gate).
pub async fn run_runtime_gate(
    activation_completed_at: DateTime<Utc>,
    evidence_path: &Path,
    effective_mode: GateMode,
) -> GateOutcome {
    if matches!(effective_mode, GateMode::Disabled) {
        return GateOutcome::Skipped {
            reason: "gate mode disabled (collector absent or operator-suppressed)".to_string(),
        };
    }

    let trigger_result = trigger_collector_with_timeout(COLLECTOR_TIMEOUT).await;
    let collector_exit: Option<i32> = match trigger_result {
        Ok(()) => None,
        Err(TriggerError::Timeout) => {
            return GateOutcome::GateError {
                reason: format!(
                    "{COLLECTOR_UNIT} did not complete within {}s",
                    COLLECTOR_TIMEOUT.as_secs()
                ),
                collector_exit_code: None,
                evidence_collected_at: None,
            };
        }
        Err(TriggerError::NonZero(code)) => {
            return GateOutcome::GateError {
                reason: format!(
                    "{COLLECTOR_UNIT} exited non-zero ({:?}); evidence may be stale",
                    code
                ),
                collector_exit_code: code,
                evidence_collected_at: None,
            };
        }
        Err(TriggerError::Spawn(err)) => {
            return GateOutcome::GateError {
                reason: format!("could not invoke systemctl: {err}"),
                collector_exit_code: None,
                evidence_collected_at: None,
            };
        }
    };
    let _: Option<i32> = collector_exit;
    let _ = effective_mode;

    let evidence = match read_evidence(evidence_path).await {
        Ok(e) => e,
        Err(err) => {
            return GateOutcome::GateError {
                reason: format!("read {}: {err}", evidence_path.display()),
                collector_exit_code: None,
                evidence_collected_at: None,
            };
        }
    };

    // LOADBEARING: evidence must be >= activation-slack - stale PASS would let rollouts promote on old data.
    let min_acceptable = activation_completed_at - chrono::Duration::seconds(TIMESTAMP_SLACK_SECS);
    if evidence.timestamp < min_acceptable {
        return GateOutcome::GateError {
            reason: format!(
                "evidence stale: collected_at={} < activation_completed_at-{}s={}",
                evidence.timestamp, TIMESTAMP_SLACK_SECS, min_acceptable
            ),
            collector_exit_code: None,
            evidence_collected_at: Some(evidence.timestamp),
        };
    }

    let failures: Vec<ControlEvidence> = evidence
        .controls
        .iter()
        .filter(|c| c.status == "non-compliant" || c.status == "error")
        .cloned()
        .collect();

    if failures.is_empty() {
        GateOutcome::Pass { evidence }
    } else {
        GateOutcome::Failures { evidence, failures }
    }
}

async fn collector_unit_present() -> bool {
    Command::new("systemctl")
        .arg("cat")
        .arg(COLLECTOR_UNIT)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

#[derive(Debug)]
enum TriggerError {
    Timeout,
    NonZero(Option<i32>),
    Spawn(anyhow::Error),
}

/// Wall-clock timeout guards against a stuck probe.
async fn trigger_collector_with_timeout(
    timeout: Duration,
) -> std::result::Result<(), TriggerError> {
    let spawn_future = Command::new("systemctl")
        .arg("start")
        .arg("--wait")
        .arg(COLLECTOR_UNIT)
        .status();
    match tokio::time::timeout(timeout, spawn_future).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(TriggerError::NonZero(status.code())),
        Ok(Err(err)) => Err(TriggerError::Spawn(
            anyhow::Error::from(err).context("spawn `systemctl start --wait`"),
        )),
        Err(_) => Err(TriggerError::Timeout),
    }
}

async fn read_evidence(path: &Path) -> Result<ComplianceEvidence> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    let parsed: ComplianceEvidence =
        serde_json::from_str(&raw).with_context(|| format!("parse JSON at {}", path.display()))?;
    Ok(parsed)
}

/// Flatten `{nis2: [...]}` to `vec!["nis2:art", ...]`; tolerates non-attrsets.
pub fn flatten_framework_articles(value: &serde_json::Value) -> Vec<String> {
    let Some(obj) = value.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    for fw in keys {
        if let Some(arts) = obj.get(fw).and_then(|v| v.as_array()) {
            for art in arts {
                if let Some(s) = art.as_str() {
                    out.push(format!("{fw}:{s}"));
                }
            }
        }
    }
    out
}

/// Bounds wire payload size; full evidence.json stays on-host.
pub fn truncate_evidence_snippet(checks: &serde_json::Value) -> serde_json::Value {
    let serialized = serde_json::to_string(checks)
        .expect("serde_json::to_string on a serde_json::Value is infallible");
    if serialized.len() <= 1024 {
        return checks.clone();
    }
    serde_json::json!({
        "_truncated_": true,
        "_original_size_bytes_": serialized.len(),
        "_preview_": serialized.chars().take(900).collect::<String>(),
    })
}

pub fn default_evidence_path() -> PathBuf {
    PathBuf::from(DEFAULT_EVIDENCE_PATH)
}

/// CP channel policy beats CLI default; falls back to auto-detect.
pub async fn resolve_runtime_gate_mode(
    persisted_mode: Option<&str>,
    cli_default: Option<&str>,
) -> GateMode {
    let cli_default_mode = cli_default
        .filter(|s| !s.is_empty() && *s != "auto")
        .map(GateMode::from_wire_str);
    let input_mode = persisted_mode
        .filter(|s| !s.is_empty() && *s != "auto")
        .map(GateMode::from_wire_str)
        .or(cli_default_mode);
    resolve_mode(input_mode).await
}

/// Returns `true` iff confirm must be skipped (host stays rolled back).
pub async fn apply_gate_outcome<R: crate::comms::Reporter>(
    gate_outcome: &GateOutcome,
    resolved_mode: GateMode,
    machine_id: &str,
    channel_ref: &str,
    reporter: &R,
    evidence_signer: &std::sync::Arc<Option<crate::evidence_signer::EvidenceSigner>>,
    activation_completed_at: DateTime<Utc>,
) -> bool {
    match gate_outcome {
        GateOutcome::Pass { .. } => {
            tracing::info!("compliance gate: PASS (all controls compliant)");
            false
        }
        GateOutcome::Skipped { reason } => {
            tracing::debug!(%reason, ?resolved_mode, "compliance gate: skipped");
            false
        }
        GateOutcome::Failures { evidence, failures } => {
            post_compliance_failures(
                failures,
                evidence,
                machine_id,
                channel_ref,
                reporter,
                evidence_signer,
            )
            .await;
            // LOADBEARING: enforce mode must actually enforce - per-control
            // events alone leave the host on a non-compliant closure.
            if resolved_mode == GateMode::Enforce {
                let reason = compliance_failure_reason(failures);
                tracing::error!(
                    %reason,
                    failure_count = failures.len(),
                    "compliance gate: failures - refusing confirm + rolling back (enforce mode)",
                );
                trigger_rollback_with_reason(
                    machine_id,
                    channel_ref,
                    reporter,
                    evidence_signer,
                    &reason,
                )
                .await;
                true
            } else {
                false
            }
        }
        GateOutcome::GateError {
            reason,
            collector_exit_code,
            evidence_collected_at,
        } => {
            post_runtime_gate_error(
                reason,
                *collector_exit_code,
                *evidence_collected_at,
                resolved_mode,
                machine_id,
                channel_ref,
                reporter,
                evidence_signer,
                activation_completed_at,
            )
            .await
        }
    }
}

fn compliance_failure_reason(failures: &[ControlEvidence]) -> String {
    let ids: Vec<&str> = failures.iter().map(|c| c.control.as_str()).collect();
    format!("compliance failures: {}", ids.join(", "))
}

async fn post_compliance_failures<R: crate::comms::Reporter>(
    failures: &[ControlEvidence],
    evidence: &ComplianceEvidence,
    machine_id: &str,
    channel_ref: &str,
    reporter: &R,
    evidence_signer: &std::sync::Arc<Option<crate::evidence_signer::EvidenceSigner>>,
) {
    use crate::evidence_signer::{sha256_jcs, try_sign, ComplianceFailureSignedPayload};
    use nixfleet_proto::agent_wire::ReportEvent;
    tracing::warn!(
        count = failures.len(),
        "compliance gate: failures - posting per-control events",
    );
    for ctrl in failures {
        let articles = flatten_framework_articles(&ctrl.framework_articles);
        let snippet = truncate_evidence_snippet(&ctrl.checks);
        let snippet_sha = sha256_jcs(&snippet).unwrap_or_default();
        let signed_payload = ComplianceFailureSignedPayload {
            hostname: machine_id,
            rollout: Some(channel_ref),
            control_id: &ctrl.control,
            status: &ctrl.status,
            framework_articles: &articles,
            evidence_collected_at: evidence.timestamp,
            evidence_snippet_sha256: snippet_sha,
        };
        let signature = evidence_signer
            .as_ref()
            .as_ref()
            .and_then(|s| try_sign(s, &signed_payload));
        reporter
            .post_report(
                Some(channel_ref),
                ReportEvent::ComplianceFailure {
                    control_id: ctrl.control.clone(),
                    status: ctrl.status.clone(),
                    framework_articles: articles,
                    evidence_snippet: Some(snippet),
                    evidence_collected_at: evidence.timestamp,
                    signature,
                },
            )
            .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn post_runtime_gate_error<R: crate::comms::Reporter>(
    reason: &str,
    collector_exit_code: Option<i32>,
    evidence_collected_at: Option<DateTime<Utc>>,
    resolved_mode: GateMode,
    machine_id: &str,
    channel_ref: &str,
    reporter: &R,
    evidence_signer: &std::sync::Arc<Option<crate::evidence_signer::EvidenceSigner>>,
    activation_completed_at: DateTime<Utc>,
) -> bool {
    use crate::evidence_signer::{try_sign, RuntimeGateErrorSignedPayload};
    use nixfleet_proto::agent_wire::ReportEvent;
    let enforcing = resolved_mode == GateMode::Enforce;
    if enforcing {
        tracing::error!(
            %reason,
            ?collector_exit_code,
            "compliance gate: ERROR - refusing confirm + rolling back (enforce mode)",
        );
    } else {
        tracing::warn!(
            %reason,
            ?collector_exit_code,
            "compliance gate: ERROR - posting event, allowing confirm (permissive mode)",
        );
    }
    let signed_payload = RuntimeGateErrorSignedPayload {
        hostname: machine_id,
        rollout: Some(channel_ref),
        reason,
        collector_exit_code,
        evidence_collected_at,
        activation_completed_at,
    };
    let signature = evidence_signer
        .as_ref()
        .as_ref()
        .and_then(|s| try_sign(s, &signed_payload));
    reporter
        .post_report(
            Some(channel_ref),
            ReportEvent::RuntimeGateError {
                reason: reason.to_string(),
                collector_exit_code,
                evidence_collected_at,
                activation_completed_at,
                signature,
            },
        )
        .await;
    if enforcing {
        trigger_rollback_with_reason(
            machine_id,
            channel_ref,
            reporter,
            evidence_signer,
            &format!("compliance gate error: {reason}"),
        )
        .await;
    }
    enforcing
}

/// Roll back to the prior generation, then report `RollbackTriggered`. The
/// reason qualifies on rollback failure so the audit chain reflects reality.
async fn trigger_rollback_with_reason<R: crate::comms::Reporter>(
    machine_id: &str,
    channel_ref: &str,
    reporter: &R,
    evidence_signer: &std::sync::Arc<Option<crate::evidence_signer::EvidenceSigner>>,
    base_reason: &str,
) {
    use crate::evidence_signer::{try_sign, RollbackTriggeredSignedPayload};
    use nixfleet_proto::agent_wire::ReportEvent;
    let rollback_result = crate::activation::rollback().await;
    let rollback_reason = match &rollback_result {
        Ok(_) => base_reason.to_string(),
        Err(err) => {
            tracing::error!(
                error = %err,
                reason = %base_reason,
                "compliance gate: rollback FAILED - host left in inconsistent state",
            );
            format!("{base_reason}; rollback FAILED: {err}")
        }
    };
    let rollback_payload = RollbackTriggeredSignedPayload {
        hostname: machine_id,
        rollout: Some(channel_ref),
        reason: &rollback_reason,
    };
    let rollback_signature = evidence_signer
        .as_ref()
        .as_ref()
        .and_then(|s| try_sign(s, &rollback_payload));
    reporter
        .post_report(
            Some(channel_ref),
            ReportEvent::RollbackTriggered {
                reason: rollback_reason,
                signature: rollback_signature,
            },
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_framework_articles_handles_attrset() {
        let v = serde_json::json!({
            "nis2": ["21(b)", "21(f)"],
            "iso27001": ["A.8.15"],
        });
        let out = flatten_framework_articles(&v);
        assert_eq!(
            out,
            vec![
                "iso27001:A.8.15".to_string(),
                "nis2:21(b)".to_string(),
                "nis2:21(f)".to_string(),
            ],
        );
    }

    #[test]
    fn flatten_framework_articles_handles_null() {
        assert!(flatten_framework_articles(&serde_json::Value::Null).is_empty());
    }

    #[test]
    fn flatten_framework_articles_handles_empty_obj() {
        assert!(flatten_framework_articles(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn truncate_evidence_snippet_returns_short_unchanged() {
        let v = serde_json::json!({"compliant": true, "x": 1});
        assert_eq!(truncate_evidence_snippet(&v), v);
    }

    #[test]
    fn truncate_evidence_snippet_truncates_large() {
        let big = "x".repeat(2000);
        let v = serde_json::json!({"compliant": false, "blob": big});
        let out = truncate_evidence_snippet(&v);
        assert_eq!(out["_truncated_"], serde_json::Value::Bool(true));
        assert!(out["_original_size_bytes_"].as_u64().unwrap() > 1024);
        assert!(out["_preview_"].as_str().unwrap().len() <= 900);
    }

    #[tokio::test]
    async fn run_runtime_gate_disabled_short_circuits_without_io() {
        // Non-existent path: Disabled must skip I/O entirely.
        let bogus = std::path::PathBuf::from("/nonexistent/evidence.json");
        let now = chrono::Utc::now();
        let outcome = run_runtime_gate(now, &bogus, GateMode::Disabled).await;
        match outcome {
            GateOutcome::Skipped { .. } => {}
            other => panic!("expected Skipped, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn evidence_parses_real_envelope() {
        let raw = r#"{
          "host": "host-05",
          "timestamp": "2026-04-29T11:57:38Z",
          "controls": [
            {
              "control": "access-control",
              "status": "compliant",
              "framework_articles": {"nis2": ["21(i)"]},
              "checks": {"compliant": true}
            },
            {
              "control": "auditLogging",
              "status": "non-compliant",
              "framework_articles": {"nis2": ["21(b)"], "iso27001": ["A.8.15"]},
              "checks": {"compliant": false, "rules": {"AL-03": {"compliant": false}}}
            }
          ],
          "overall": "1/2 controls compliant"
        }"#;
        let evidence: ComplianceEvidence = serde_json::from_str(raw).unwrap();
        assert_eq!(evidence.host, "host-05");
        assert_eq!(evidence.controls.len(), 2);
        let failures: Vec<_> = evidence
            .controls
            .iter()
            .filter(|c| c.status == "non-compliant")
            .collect();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].control, "auditLogging");
    }
}
