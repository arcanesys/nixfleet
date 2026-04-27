//! `fleet.resolved.json` types. Produced by CI's Nix eval, consumed by CP and
//! (fallback path) agents; JCS bytes must round-trip identically across Nix + Rust.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FleetResolved {
    pub schema_version: u32,
    pub hosts: HashMap<String, Host>,
    pub channels: HashMap<String, Channel>,
    #[serde(default)]
    pub rollout_policies: HashMap<String, RolloutPolicy>,
    pub waves: HashMap<String, Vec<Wave>>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    /// Cross-channel ordering: a `before` channel must reach Converged before
    /// any new rollout opens on the `after` channel. Within-channel coordination
    /// uses `edges`. Cycles rejected at mkFleet eval time.
    #[serde(default)]
    pub channel_edges: Vec<ChannelEdge>,
    #[serde(default)]
    pub disruption_budgets: Vec<DisruptionBudget>,
    pub meta: Meta,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Host {
    pub system: String,
    pub tags: Vec<String>,
    pub channel: String,
    #[serde(default)]
    pub closure_hash: Option<String>,
    #[serde(default)]
    pub pubkey: Option<String>,
    /// Operator-declared commit pin. Resolved at mkFleet eval time from the
    /// most-specific declaration in the host > tag > channel chain; populated
    /// only when the effective pin is non-empty AND unexpired. When present,
    /// `nixfleet-release` builds from `pin.commit` instead of the release commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<Pin>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    /// Source-control rev the host's closure should be built from. Opaque to
    /// the framework; typically a 40-char SHA but short SHAs + tag names work.
    pub commit: String,
    /// Free-form operator note. Not parsed; surfaced verbatim in
    /// `nixfleet status` + dashboards.
    pub reason: String,
    /// Hard expiry. Expired pins are filtered at mkFleet eval time, so when
    /// present here the artifact already passed the filter at signing time -
    /// informational for operators reading the JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub rollout_policy: String,
    pub reconcile_interval_minutes: u32,
    /// MINUTES despite missing `_minutes` suffix (kept for wire-compat).
    /// Convert via [`Channel::freshness_window_duration`].
    pub freshness_window: u32,
    pub signing_interval_minutes: u32,
    pub compliance: Compliance,
}

impl Channel {
    /// Helper that converts minutes -> Duration. Avoids the
    /// `Duration::from_secs(raw)` 60× landmine.
    pub fn freshness_window_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.freshness_window as u64 * 60)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Compliance {
    pub frameworks: Vec<String>,
    /// `disabled` / `permissive` / `enforce`. Default `enforce`.
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RolloutPolicy {
    pub strategy: String,
    pub waves: Vec<PolicyWave>,
    #[serde(default)]
    pub health_gate: HealthGate,
    pub on_health_failure: OnHealthFailure,
}

/// Recovery action when a host fails its health gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnHealthFailure {
    /// Stop advancing; failed host stays Failed pending operator action.
    Halt,
    /// Roll the failed host back to its previous closure, then halt.
    RollbackAndHalt,
}

impl std::fmt::Display for OnHealthFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            OnHealthFailure::Halt => "halt",
            OnHealthFailure::RollbackAndHalt => "rollback-and-halt",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PolicyWave {
    pub selector: Selector,
    pub soak_minutes: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Selector {
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub tags_any: Vec<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub all: bool,
}

impl Selector {
    /// Match a single host. Mirrors `lib/mk-fleet.nix:resolveSelector`: any
    /// rule that fires (all / hosts / channel / tags-all / tags-any) matches.
    /// Sub-selector composition (and / not) is mkFleet-only and not exposed
    /// in the wire format.
    pub fn matches(&self, host_name: &str, host: &Host) -> bool {
        if self.all {
            return true;
        }
        if !self.hosts.is_empty() && self.hosts.iter().any(|h| h == host_name) {
            return true;
        }
        if let Some(ch) = &self.channel {
            if &host.channel == ch {
                return true;
            }
        }
        if !self.tags.is_empty() && self.tags.iter().all(|t| host.tags.contains(t)) {
            return true;
        }
        if !self.tags_any.is_empty() && self.tags_any.iter().any(|t| host.tags.contains(t)) {
            return true;
        }
        false
    }

    /// Resolve to matching host names. Order follows the input iterator;
    /// callers that need a stable ordering should sort.
    pub fn resolve<'a, I: IntoIterator<Item = (&'a String, &'a Host)>>(
        &self,
        hosts: I,
    ) -> Vec<String> {
        hosts
            .into_iter()
            .filter(|(n, h)| self.matches(n, h))
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// Canonical short string for log lines, metric labels, and any consumer
    /// that needs to refer to a `Selector` by name. Sorted-list semantics keep
    /// rendering stable across HashMap iteration orders.
    pub fn summary(&self) -> String {
        if self.all {
            return "all".to_string();
        }
        if let Some(channel) = &self.channel {
            return format!("channel:{channel}");
        }
        if !self.tags.is_empty() {
            let mut t = self.tags.clone();
            t.sort();
            return format!("tags:{}", t.join(","));
        }
        if !self.tags_any.is_empty() {
            let mut t = self.tags_any.clone();
            t.sort();
            return format!("tags_any:{}", t.join(","));
        }
        if !self.hosts.is_empty() {
            let mut h = self.hosts.clone();
            h.sort();
            return format!("hosts:{}", h.join(","));
        }
        "unknown".to_string()
    }
}

// Nix emits `"healthGate": {}` when no inner constraints set; the
// skip_serializing_none below preserves that empty-object shape for JCS parity.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HealthGate {
    #[serde(default)]
    pub systemd_failed_units: Option<SystemdFailedUnits>,
    #[serde(default)]
    pub compliance_probes: Option<ComplianceProbes>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SystemdFailedUnits {
    pub max: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ComplianceProbes {
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Wave {
    pub hosts: Vec<String>,
    pub soak_minutes: u32,
}

/// Per-host DAG edge: `gated` host dispatches only once `gates` host reaches
/// terminal-for-ordering (Soaked / Converged) within the same rollout. Both
/// hosts must be on the same channel; cross-channel ordering is `ChannelEdge`'s
/// job. Hard cutover from pre-rename `before`/`after` field names - those
/// bytes will not parse.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    /// Host whose dispatch is held until `gates` completes.
    pub gated: String,
    /// Host that must reach Soaked/Converged before `gated` can dispatch.
    pub gates: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Cross-channel ordering edge. The `gates` channel's most-recent rollout
/// must reach terminal `converged` before any new rollout opens on `gated`.
/// If `gates` has never had a rollout, the gate is open. Validated at
/// mkFleet eval time: both channels must exist, no cycles. Pre-rename
/// `before`/`after` wire keys accepted via serde alias so older signed
/// bytes still verify on upgraded CPs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChannelEdge {
    /// Predecessor channel. Was `before`; kept as serde alias.
    #[serde(alias = "before")]
    pub gates: String,
    /// Dependent channel, held until `gates` converges. Was `after`.
    #[serde(alias = "after")]
    pub gated: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DisruptionBudget {
    /// Tag-driven selector resolved at reconcile time so tag membership can
    /// change without re-signing fleet.resolved.
    pub selector: Selector,
    #[serde(default)]
    pub max_in_flight: Option<u32>,
    #[serde(default)]
    pub max_in_flight_pct: Option<u32>,
}

// LOADBEARING: signed_at + ci_commit serialize as `null` (no skip) to match
// the Nix evaluator's shape - JCS byte-identity depends on it. Only
// `signature_algorithm` is genuinely optional in the wire format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub schema_version: u32,
    #[serde(default)]
    pub signed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ci_commit: Option<String>,
    /// Absent ≡ "ed25519" at schemaVersion=1 (CONTRACTS §V Pattern A). Pre-stamp
    /// eval emits absent; `stamp_meta` populates at signing time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_algorithm: Option<String>,
}

impl Meta {
    /// `signature_algorithm` with the `absent ≡ "ed25519"` rule applied.
    pub fn signature_algorithm_or_default(&self) -> &str {
        self.signature_algorithm.as_deref().unwrap_or("ed25519")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pre-rename fleet.resolved bytes used `before`/`after` keys. New CPs
    /// must accept those via serde alias, otherwise an upgraded CP would
    /// reject any signed artifact in the channel-refs window.
    #[test]
    fn channel_edge_accepts_legacy_before_after_wire_format() {
        let legacy = r#"{"before":"edge","after":"stable","reason":"test canary"}"#;
        let parsed: ChannelEdge = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.gates, "edge");
        assert_eq!(parsed.gated, "stable");
        assert_eq!(parsed.reason.as_deref(), Some("test canary"));
    }

    /// New emitters write `gates`/`gated`; round-trip must be lossless.
    #[test]
    fn channel_edge_canonical_wire_format_round_trips() {
        let edge = ChannelEdge {
            gates: "edge".into(),
            gated: "stable".into(),
            reason: Some("canary".into()),
        };
        let bytes = serde_json::to_string(&edge).unwrap();
        assert!(
            bytes.contains("\"gates\":\"edge\""),
            "wire must use canonical 'gates' field; got {bytes}"
        );
        assert!(
            bytes.contains("\"gated\":\"stable\""),
            "wire must use canonical 'gated' field; got {bytes}"
        );
        let back: ChannelEdge = serde_json::from_str(&bytes).unwrap();
        assert_eq!(back, edge);
    }

    #[test]
    fn selector_summary_priority_and_sorted_lists() {
        let s = Selector {
            all: true,
            ..Default::default()
        };
        assert_eq!(s.summary(), "all");

        let s = Selector {
            channel: Some("stable".into()),
            ..Default::default()
        };
        assert_eq!(s.summary(), "channel:stable");

        let s = Selector {
            tags: vec!["server".into(), "prod".into()],
            ..Default::default()
        };
        assert_eq!(s.summary(), "tags:prod,server");

        let s = Selector {
            tags_any: vec!["b".into(), "a".into()],
            ..Default::default()
        };
        assert_eq!(s.summary(), "tags_any:a,b");

        let s = Selector {
            hosts: vec!["zzz".into(), "aaa".into()],
            ..Default::default()
        };
        assert_eq!(s.summary(), "hosts:aaa,zzz");

        // Explicit "unknown" sentinel keeps a Prometheus label queryable.
        assert_eq!(Selector::default().summary(), "unknown");
    }
}
