//! Observed-state types. CP projects SQLite state into these per tick;
//! reconciler never mutates them.

use crate::host_state::HostRolloutState;
use crate::rollout_state::RolloutState;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::str::FromStr;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Observed {
    pub channel_refs: HashMap<String, String>,
    pub last_rolled_refs: HashMap<String, String>,
    pub host_state: HashMap<String, HostState>,
    pub active_rollouts: Vec<Rollout>,
    /// `[rollout_id][host] → count` of outstanding evidence failures.
    /// Aggregates both `ComplianceFailure` and `RuntimeGateError` (single
    /// DB-side filter at `db::reports::outstanding_compliance_events_by_rollout`).
    /// Per-rollout grouping enforces resolution-by-replacement so events under
    /// a superseded rollout don't gate the new one.
    #[serde(default)]
    pub outstanding_compliance_events_by_rollout: HashMap<String, HashMap<String, usize>>,
    /// Last `RolloutDeferred` journalled per channel. The reconciler debounces
    /// re-emission against this; without it every blocked tick would pollute
    /// the journal with an identical line.
    #[serde(default)]
    pub last_deferrals: HashMap<String, DeferralRecord>,
    /// Per-host probe-pass state from the latest checkin. Hosts absent from
    /// the map default to `true` so a misconfigured projection can't stall
    /// every promotion (gate fails open).
    #[serde(default)]
    pub host_probes_passing: HashMap<String, bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeferralRecord {
    pub target_ref: String,
    pub blocked_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostState {
    pub online: bool,
    #[serde(default)]
    pub current_generation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rollout {
    pub id: String,
    pub channel: String,
    pub target_ref: String,
    /// Serde shim: wire is string, in-memory is typed enum.
    #[serde(
        serialize_with = "serialize_rollout_state",
        deserialize_with = "deserialize_rollout_state"
    )]
    pub state: RolloutState,
    pub current_wave: usize,
    #[serde(
        serialize_with = "serialize_host_states_map",
        deserialize_with = "deserialize_host_states_map"
    )]
    pub host_states: HashMap<String, HostRolloutState>,
    /// `now - last_healthy_since[host] >= wave.soak_minutes` ⇒ soaked.
    /// Hosts not in Healthy are absent.
    #[serde(default)]
    pub last_healthy_since: HashMap<String, DateTime<Utc>>,
    /// Disruption-budget snapshot frozen at projection time, so mid-rollout
    /// retag does not reshape enforcement. Cross-rollout in-flight summing
    /// matches by `selector` equality.
    #[serde(default)]
    pub budgets: Vec<nixfleet_proto::RolloutBudget>,
    /// `Some(t)` after `ConvergeRollout` (or orphan sweep) marked the rollout
    /// terminal; `None` while still progressing through waves. Visible to the
    /// reconciler (short-circuits advance) and to `channel_edges` (lets the
    /// successor release symmetrically in both gate modes).
    #[serde(default)]
    pub terminal_at: Option<DateTime<Utc>>,
}

impl Rollout {
    /// Active for `channelEdges` / host-edges sequencing. Empty
    /// `host_states` counts as active (work declared, not started).
    /// `Failed` / `Reverted` count as active - predecessor in trouble holds
    /// the successor. See `HostRolloutState::is_terminal_for_ordering`.
    pub fn is_active_for_ordering(&self) -> bool {
        if self.host_states.is_empty() {
            return true;
        }
        !self
            .host_states
            .values()
            .all(HostRolloutState::is_terminal_for_ordering)
    }
}

fn serialize_rollout_state<S: Serializer>(s: &RolloutState, ser: S) -> Result<S::Ok, S::Error> {
    ser.serialize_str(s.as_str())
}

fn deserialize_rollout_state<'de, D: Deserializer<'de>>(de: D) -> Result<RolloutState, D::Error> {
    let s = String::deserialize(de)?;
    RolloutState::from_str(&s).map_err(serde::de::Error::custom)
}

fn serialize_host_states_map<S: Serializer>(
    map: &HashMap<String, HostRolloutState>,
    ser: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;
    let mut m = ser.serialize_map(Some(map.len()))?;
    for (k, v) in map {
        m.serialize_entry(k, v.as_db_str())?;
    }
    m.end()
}

fn deserialize_host_states_map<'de, D: Deserializer<'de>>(
    de: D,
) -> Result<HashMap<String, HostRolloutState>, D::Error> {
    let raw = HashMap::<String, String>::deserialize(de)?;
    raw.into_iter()
        .map(|(k, v)| {
            HostRolloutState::from_db_str(&v)
                .map(|s| (k, s))
                .map_err(serde::de::Error::custom)
        })
        .collect()
}
