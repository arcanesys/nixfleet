//! `fleet.resolved.json`. Produced by CI's Nix eval, consumed by the CP
//! and (fallback path) agents. Byte-identical JCS bytes across Nix + Rust.

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
    /// Cross-channel ordering: a `before` channel must reach Converged
    /// before any new rollout opens on the `after` channel. RFC-0002 §4.3
    /// — within-channel coordination uses `edges`; channel-level uses this.
    /// Cycles are rejected at mkFleet eval time.
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub rollout_policy: String,
    pub reconcile_interval_minutes: u32,
    /// MINUTES (despite missing `_minutes` suffix — kept for wire-compat).
    /// Convert via [`Channel::freshness_window_duration`].
    pub freshness_window: u32,
    pub signing_interval_minutes: u32,
    pub compliance: Compliance,
}

impl Channel {
    /// `freshness_window` is MINUTES; this helper avoids the
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
    /// Match a single host. Mirrors `lib/mk-fleet.nix:resolveSelector` —
    /// any rule that fires (all / hosts / channel / tags-all / tags-any)
    /// matches; sub-selector composition (and / not) is mkFleet-only and
    /// not exposed in the wire format.
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

    /// Resolve to the matching host names. Order is `fleet.hosts`'s natural
    /// iteration; callers that need a stable ordering should sort.
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

    /// Canonical short string for log lines, metric labels, and any
    /// other consumer that needs to refer to a `Selector` by name.
    /// Sorted-list semantics keep the rendering stable across HashMap
    /// iteration orders. Priority order matches `matches()` evaluation
    /// shape: broadest predicate (`all`) first, then specific lists.
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

// GOTCHA: Nix emits `"healthGate": {}` when no inner constraints set;
// skip_serializing_none preserves that empty-object shape.
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

/// Per-host DAG edge: `gated` host can dispatch only once `gates` host
/// reaches terminal-for-ordering (Soaked / Converged) within the same
/// rollout. Both hosts must be on the same channel — cross-channel
/// ordering is `ChannelEdge`'s job (the gate operates within a single
/// rollout's host_states; cross-channel pairs would silently brick the
/// gated host).
///
/// Schema note: pre-rev these were named `before`/`after`, where
/// "before" actually meant "the gated host" (held back) — the
/// opposite of natural DAG reading. Renamed to `gated`/`gates` for
/// clarity. Hard cutover: any pre-rename fleet.resolved bytes will
/// fail to parse against this schema. Verified safe because lab is
/// the only CP and gets the new code + new artifact together.
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

/// Cross-channel ordering edge. `before` channel must converge before any
/// rollout opens on `after`. "Converge" = the most-recent rollout on `before`
/// reached terminal state `converged`. If `before` has never had a rollout,
/// the gate is open (no rollout to wait for). Validated at mkFleet eval time:
/// both channels must exist, no cycles.
/// Field semantics match the host-level `Edge` (gated/gates):
///
///   `gates` is the predecessor channel — it must converge before any
///   new rollout opens on `gated`. "gates" reads as "the channel that
///   gates dispatch on `gated`." `gated` reads as "the channel whose
///   dispatch is gated by `gates`."
///
/// Wire-format aliases `before`/`after` accepted on deserialize so
/// fleet.resolved bytes signed before the rename still verify on
/// upgraded CPs. New emitters (mk-fleet) write `gates`/`gated`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChannelEdge {
    /// Predecessor channel. Was `before` — kept as serde alias.
    #[serde(alias = "before")]
    pub gates: String,
    /// Dependent channel — held until `gates` converges. Was `after`.
    #[serde(alias = "after")]
    pub gated: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DisruptionBudget {
    /// Tag-driven selector resolved at reconcile time so adding/removing
    /// hosts under a tag doesn't require re-signing fleet.resolved.
    pub selector: Selector,
    #[serde(default)]
    pub max_in_flight: Option<u32>,
    #[serde(default)]
    pub max_in_flight_pct: Option<u32>,
}

// LOADBEARING: signed_at + ci_commit serialize as `null` (no skip) to match
// the Nix evaluator's shape — JCS byte-identity round-trip depends on it.
// Only `signature_algorithm` is genuinely optional in the wire format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub schema_version: u32,
    #[serde(default)]
    pub signed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ci_commit: Option<String>,
    /// CONTRACTS §V Pattern A: absent ≡ "ed25519" at schemaVersion=1. Pre-stamp
    /// eval emits absent; `stamp_meta` populates at signing time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_algorithm: Option<String>,
}

impl Meta {
    /// `signature_algorithm` with the `absent ≡ "ed25519"` rule applied.
    /// Use this in any read path that needs a concrete algorithm string.
    pub fn signature_algorithm_or_default(&self) -> &str {
        self.signature_algorithm.as_deref().unwrap_or("ed25519")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pre-rename fleet.resolved bytes used `before`/`after` keys.
    /// New CPs must accept those via serde alias, otherwise an
    /// upgraded CP would reject any signed artifact in the existing
    /// channel-refs window.
    #[test]
    fn channel_edge_accepts_legacy_before_after_wire_format() {
        let legacy = r#"{"before":"edge","after":"stable","reason":"lab canary"}"#;
        let parsed: ChannelEdge = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.gates, "edge");
        assert_eq!(parsed.gated, "stable");
        assert_eq!(parsed.reason.as_deref(), Some("lab canary"));
    }

    /// New emitters (mk-fleet post-rename) write `gates`/`gated`.
    /// Round-trip must be lossless.
    #[test]
    fn channel_edge_canonical_wire_format_round_trips() {
        let edge = ChannelEdge {
            gates: "edge".into(),
            gated: "stable".into(),
            reason: Some("canary".into()),
        };
        let bytes = serde_json::to_string(&edge).unwrap();
        // Canonical wire emits new field names.
        assert!(bytes.contains("\"gates\":\"edge\""), "wire must use canonical 'gates' field; got {bytes}");
        assert!(bytes.contains("\"gated\":\"stable\""), "wire must use canonical 'gated' field; got {bytes}");
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
            // Unsorted on the way in; summary sorts.
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

        // Empty selector: explicit sentinel rather than empty string,
        // so a Prometheus label with this value is still queryable.
        assert_eq!(Selector::default().summary(), "unknown");
    }
}
