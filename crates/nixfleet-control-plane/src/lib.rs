//! NixFleet control plane — Phase 2 (read-only reconciler runner).
//!
//! Per the architecture doc Phase 2: ship the reconciler from the spike,
//! run it as a systemd timer on the M70q, read `fleet.resolved.json` +
//! a simulated `observed.json`, print the action plan to the journal.
//! No actions taken, no agents yet — just planning.
//!
//! This crate exposes [`tick`] as a pure function (testable) and
//! [`render_plan`] as a JSON-line emitter for the systemd journal. The
//! binary in `src/main.rs` is the thin CLI shell.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use nixfleet_proto::TrustConfig;
use nixfleet_reconciler::{reconcile, verify_artifact, Action, Observed, VerifyError};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct TickInputs {
    pub artifact_path: PathBuf,
    pub signature_path: PathBuf,
    pub trust_path: PathBuf,
    pub observed_path: PathBuf,
    pub now: DateTime<Utc>,
    pub freshness_window: Duration,
}

#[derive(Debug)]
pub struct TickOutput {
    pub now: DateTime<Utc>,
    pub verify: VerifyOutcome,
}

#[derive(Debug)]
pub enum VerifyOutcome {
    Ok {
        signed_at: DateTime<Utc>,
        ci_commit: Option<String>,
        observed: Observed,
        actions: Vec<Action>,
    },
    Failed {
        reason: String,
    },
}

pub fn tick(inputs: &TickInputs) -> anyhow::Result<TickOutput> {
    let artifact = fs::read(&inputs.artifact_path)
        .map_err(|e| anyhow::anyhow!("read artifact {}: {e}", inputs.artifact_path.display()))?;
    let signature = fs::read(&inputs.signature_path)
        .map_err(|e| anyhow::anyhow!("read signature {}: {e}", inputs.signature_path.display()))?;
    let trust_raw = fs::read_to_string(&inputs.trust_path)
        .map_err(|e| anyhow::anyhow!("read trust {}: {e}", inputs.trust_path.display()))?;
    let trust: TrustConfig = serde_json::from_str(&trust_raw)
        .map_err(|e| anyhow::anyhow!("parse trust {}: {e}", inputs.trust_path.display()))?;

    let trusted_keys = trust.ci_release_key.active_keys();
    let reject_before = trust.ci_release_key.reject_before;

    let verify = match verify_artifact(
        &artifact,
        &signature,
        &trusted_keys,
        inputs.now,
        inputs.freshness_window,
        reject_before,
    ) {
        Ok(fleet) => {
            let signed_at = fleet
                .meta
                .signed_at
                .expect("verified artifact carries meta.signedAt by §4 contract");
            let ci_commit = fleet.meta.ci_commit.clone();

            let observed_raw = fs::read_to_string(&inputs.observed_path).map_err(|e| {
                anyhow::anyhow!("read observed {}: {e}", inputs.observed_path.display())
            })?;
            let observed: Observed = serde_json::from_str(&observed_raw).map_err(|e| {
                anyhow::anyhow!("parse observed {}: {e}", inputs.observed_path.display())
            })?;

            let actions = reconcile(&fleet, &observed, inputs.now);

            VerifyOutcome::Ok {
                signed_at,
                ci_commit,
                observed,
                actions,
            }
        }
        Err(err) => VerifyOutcome::Failed {
            reason: classify_verify_error(&err),
        },
    };

    Ok(TickOutput {
        now: inputs.now,
        verify,
    })
}

fn classify_verify_error(err: &VerifyError) -> String {
    match err {
        VerifyError::Parse(_) => "parse".into(),
        VerifyError::BadSignature => "bad-signature".into(),
        VerifyError::NotSigned => "unsigned".into(),
        VerifyError::Stale { .. } => "stale".into(),
        VerifyError::RejectedBeforeTimestamp { .. } => "reject-before".into(),
        VerifyError::SchemaVersionUnsupported(_) => "schema-version".into(),
        VerifyError::Canonicalize(_) => "canonicalize".into(),
        VerifyError::UnsupportedAlgorithm { .. } => "unsupported-algorithm".into(),
        VerifyError::BadPubkeyEncoding { .. } => "bad-pubkey".into(),
        VerifyError::NoTrustRoots => "no-trust-roots".into(),
    }
}

/// Render a tick result as one summary JSON line plus one JSON line per
/// action. Each line is intended for the systemd journal — `journalctl
/// -o cat` produces the raw JSON; `jq` filters trivially.
pub fn render_plan(out: &TickOutput) -> String {
    let mut s = String::new();
    s.push_str(&render_summary(out));
    s.push('\n');
    if let VerifyOutcome::Ok { actions, .. } = &out.verify {
        for action in actions {
            s.push_str(&serde_json::to_string(action).expect("Action serialises"));
            s.push('\n');
        }
    }
    s
}

fn render_summary(out: &TickOutput) -> String {
    match &out.verify {
        VerifyOutcome::Ok {
            signed_at,
            ci_commit,
            observed,
            actions,
        } => json!({
            "event": "tick",
            "verify_ok": true,
            "now": out.now.to_rfc3339(),
            "signed_at": signed_at.to_rfc3339(),
            "ci_commit": ci_commit,
            "channels_observed": observed.channel_refs.len(),
            "active_rollouts": observed.active_rollouts.len(),
            "actions": actions.len(),
        })
        .to_string(),
        VerifyOutcome::Failed { reason } => json!({
            "event": "tick",
            "verify_ok": false,
            "now": out.now.to_rfc3339(),
            "reason": reason,
        })
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn observed_no_rollouts() -> Observed {
        Observed {
            channel_refs: HashMap::from([("stable".to_string(), "abc123".to_string())]),
            last_rolled_refs: HashMap::new(),
            host_state: HashMap::new(),
            active_rollouts: vec![],
        }
    }

    #[test]
    fn render_summary_verify_ok_shape() {
        let out = TickOutput {
            now: DateTime::parse_from_rfc3339("2026-04-25T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            verify: VerifyOutcome::Ok {
                signed_at: DateTime::parse_from_rfc3339("2026-04-25T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
                ci_commit: Some("deadbeef".into()),
                observed: observed_no_rollouts(),
                actions: vec![Action::OpenRollout {
                    channel: "stable".into(),
                    target_ref: "abc123".into(),
                }],
            },
        };
        let summary = render_summary(&out);
        let v: serde_json::Value = serde_json::from_str(&summary).unwrap();
        assert_eq!(v["event"], "tick");
        assert_eq!(v["verify_ok"], true);
        assert_eq!(v["actions"], 1);
        assert_eq!(v["ci_commit"], "deadbeef");
    }

    #[test]
    fn render_summary_verify_failed_shape() {
        let out = TickOutput {
            now: Utc::now(),
            verify: VerifyOutcome::Failed {
                reason: "stale".into(),
            },
        };
        let summary = render_summary(&out);
        let v: serde_json::Value = serde_json::from_str(&summary).unwrap();
        assert_eq!(v["verify_ok"], false);
        assert_eq!(v["reason"], "stale");
    }

    #[test]
    fn render_plan_emits_one_line_per_action_plus_summary() {
        let out = TickOutput {
            now: Utc::now(),
            verify: VerifyOutcome::Ok {
                signed_at: Utc::now(),
                ci_commit: None,
                observed: observed_no_rollouts(),
                actions: vec![
                    Action::OpenRollout {
                        channel: "stable".into(),
                        target_ref: "abc".into(),
                    },
                    Action::Skip {
                        host: "krach".into(),
                        reason: "offline".into(),
                    },
                ],
            },
        };
        let body = render_plan(&out);
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 3, "one summary + two actions");

        let summary: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(summary["event"], "tick");

        let action0: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(action0["action"], "open_rollout");

        let action1: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(action1["action"], "skip");
    }
}
