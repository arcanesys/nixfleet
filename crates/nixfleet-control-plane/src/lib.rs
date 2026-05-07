#![allow(clippy::doc_lazy_continuation)]
//! NixFleet control plane: TLS server + reconciler.

pub mod auth;
pub mod db;
pub mod deferrals_view;
pub mod dispatch;
pub mod metrics;
pub mod observed_projection;
pub mod observed_view;
pub mod polling;
pub mod rollouts_source;
pub mod server;
pub mod state;
pub mod state_view;
pub mod timers;
pub mod tls;

#[cfg(test)]
mod lifecycle_parity_tests;

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
    Ok(Box<VerifyOk>),
    Failed { reason: String },
}

#[derive(Debug)]
pub struct VerifyOk {
    pub signed_at: DateTime<Utc>,
    pub ci_commit: Option<String>,
    pub observed: Observed,
    pub actions: Vec<Action>,
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

    // Time-aware: includes `successor` during the rotation overlap
    // window (`now < retire_at`). Outside the window, identical to
    // `active_keys()`.
    let trusted_keys = trust.ci_release_key.active_keys_at(inputs.now);
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
            let signed_at = fleet.meta.signed_at.ok_or_else(|| {
                anyhow::anyhow!(
                    "verified artifact lacks meta.signedAt despite §4 contract — verify layer bug",
                )
            })?;
            let ci_commit = fleet.meta.ci_commit.clone();

            let observed_raw = fs::read_to_string(&inputs.observed_path).map_err(|e| {
                anyhow::anyhow!("read observed {}: {e}", inputs.observed_path.display())
            })?;
            let observed: Observed = serde_json::from_str(&observed_raw).map_err(|e| {
                anyhow::anyhow!("parse observed {}: {e}", inputs.observed_path.display())
            })?;

            let mut actions = reconcile(&fleet, &observed, inputs.now);
            // Append RotateTrustRoot informational signals when a slot's
            // retire_at deadline has passed and a successor is declared.
            // Pure function, idempotent across ticks until the operator
            // mutates fleet.nix's trust block.
            actions.extend(nixfleet_reconciler::check_trust_rotations(
                &trust, inputs.now,
            ));

            VerifyOutcome::Ok(Box::new(VerifyOk {
                signed_at,
                ci_commit,
                observed,
                actions,
            }))
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
        VerifyError::FutureDated { .. } => "future-dated".into(),
        VerifyError::RejectedBeforeTimestamp { .. } => "reject-before".into(),
        VerifyError::SchemaVersionUnsupported(_) => "schema-version".into(),
        VerifyError::Canonicalize(_) => "canonicalize".into(),
        VerifyError::UnsupportedAlgorithm { .. } => "unsupported-algorithm".into(),
        VerifyError::BadPubkeyEncoding { .. } => "bad-pubkey".into(),
        VerifyError::NoTrustRoots => "no-trust-roots".into(),
    }
}

/// Render a tick as one summary JSON line plus one JSON line per action; offline `Skip` actions coalesce into one `skip_summary`.
pub fn render_plan(out: &TickOutput) -> String {
    let mut s = String::new();
    s.push_str(&render_summary(out));
    s.push('\n');
    if let VerifyOutcome::Ok(ok) = &out.verify {
        let mut offline_hosts: Vec<&str> = Vec::new();
        for action in &ok.actions {
            if let Action::Skip { host, reason } = action {
                if reason == "offline" {
                    offline_hosts.push(host.as_str());
                    continue;
                }
            }
            s.push_str(&serde_json::to_string(action).expect("Action serialises"));
            s.push('\n');
        }
        if !offline_hosts.is_empty() {
            // Reconciler emits one Skip-offline per (rollout, host); summary wants distinct hosts.
            offline_hosts.sort_unstable();
            offline_hosts.dedup();
            s.push_str(
                &json!({
                    "action": "skip_summary",
                    "reason": "offline",
                    "hosts": offline_hosts,
                })
                .to_string(),
            );
            s.push('\n');
        }
    }
    s
}

fn render_summary(out: &TickOutput) -> String {
    match &out.verify {
        VerifyOutcome::Ok(ok) => json!({
            "event": "tick",
            "verify_ok": true,
            "now": out.now.to_rfc3339(),
            "signed_at": ok.signed_at.to_rfc3339(),
            "ci_commit": ok.ci_commit,
            "channels_observed": ok.observed.channel_refs.len(),
            "active_rollouts": ok.observed.active_rollouts.len(),
            "actions": ok.actions.len(),
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
            outstanding_compliance_events_by_rollout: HashMap::new(),
            last_deferrals: HashMap::new(),
            host_probes_passing: HashMap::new(),
        }
    }

    #[test]
    fn render_summary_verify_ok_shape() {
        let out = TickOutput {
            now: DateTime::parse_from_rfc3339("2026-04-25T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            verify: VerifyOutcome::Ok(Box::new(VerifyOk {
                signed_at: DateTime::parse_from_rfc3339("2026-04-25T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
                ci_commit: Some("deadbeef".into()),
                observed: observed_no_rollouts(),
                actions: vec![Action::OpenRollout {
                    channel: "stable".into(),
                    target_ref: "abc123".into(),
                }],
            })),
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
            verify: VerifyOutcome::Ok(Box::new(VerifyOk {
                signed_at: Utc::now(),
                ci_commit: None,
                observed: observed_no_rollouts(),
                actions: vec![
                    Action::OpenRollout {
                        channel: "stable".into(),
                        target_ref: "abc".into(),
                    },
                    Action::Skip {
                        host: "test-host".into(),
                        reason: "offline".into(),
                    },
                ],
            })),
        };
        let body = render_plan(&out);
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 3, "one summary + open_rollout + skip_summary");

        let summary: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(summary["event"], "tick");

        let action0: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(action0["action"], "open_rollout");

        let action1: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(action1["action"], "skip_summary");
        assert_eq!(action1["reason"], "offline");
        assert_eq!(action1["hosts"], serde_json::json!(["test-host"]));
    }

    #[test]
    fn render_plan_offline_skips_coalesce_other_skips_keep_per_line() {
        let out = TickOutput {
            now: Utc::now(),
            verify: VerifyOutcome::Ok(Box::new(VerifyOk {
                signed_at: Utc::now(),
                ci_commit: None,
                observed: observed_no_rollouts(),
                actions: vec![
                    Action::OpenRollout {
                        channel: "stable".into(),
                        target_ref: "abc".into(),
                    },
                    Action::Skip {
                        host: "host-a".into(),
                        reason: "offline".into(),
                    },
                    Action::Skip {
                        host: "host-b".into(),
                        reason: "offline".into(),
                    },
                    Action::Skip {
                        host: "host-c".into(),
                        reason: "disruption budget (1/1 in flight)".into(),
                    },
                ],
            })),
        };
        let body = render_plan(&out);
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 4);
        let summary_action: serde_json::Value = serde_json::from_str(lines[3]).unwrap();
        assert_eq!(summary_action["action"], "skip_summary");
        assert_eq!(summary_action["reason"], "offline");
        assert_eq!(
            summary_action["hosts"],
            serde_json::json!(["host-a", "host-b"])
        );
    }

    #[test]
    fn render_plan_skip_summary_dedups_across_rollouts() {
        let actions: Vec<Action> = (0..14)
            .flat_map(|_| {
                ["host-a", "host-b", "host-c"].iter().map(|h| Action::Skip {
                    host: (*h).to_string(),
                    reason: "offline".into(),
                })
            })
            .collect();
        let out = TickOutput {
            now: Utc::now(),
            verify: VerifyOutcome::Ok(Box::new(VerifyOk {
                signed_at: Utc::now(),
                ci_commit: None,
                observed: observed_no_rollouts(),
                actions,
            })),
        };
        let body = render_plan(&out);
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "expected 1 summary + 1 skip_summary, got: {body}"
        );
        let summary_action: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(
            summary_action["hosts"],
            serde_json::json!(["host-a", "host-b", "host-c"]),
            "hosts must be deduped despite 14 rollouts × 3 hosts of input",
        );
    }
}
