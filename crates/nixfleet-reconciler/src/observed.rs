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
    /// `[rollout_id][host] → count` of outstanding compliance evidence
    /// failures. Aggregates BOTH `ComplianceFailure` events (a probe
    /// returned FAIL) and `RuntimeGateError` events (the collector
    /// itself broke / evidence is stale) — both classes mean "this
    /// host's evidence chain is broken", and both block wave promotion
    /// identically under enforce mode (DB-side filter at
    /// `db::reports::outstanding_compliance_events_by_rollout`).
    /// Per-rollout grouping enforces resolution-by-replacement so
    /// events under a superseded rollout don't gate the new one.
    #[serde(default)]
    pub outstanding_compliance_events_by_rollout: HashMap<String, HashMap<String, usize>>,
    /// Last `RolloutDeferred` the CP successfully journalled per channel.
    /// The reconciler consults this and only emits a fresh `RolloutDeferred`
    /// when (target_ref, blocked_by) would change — without this debounce,
    /// every reconcile tick on a blocked channel would pollute the journal
    /// with an identical line.
    #[serde(default)]
    pub last_deferrals: HashMap<String, DeferralRecord>,
    /// Issue #86: per-host probe-pass state extracted from each host's
    /// latest checkin. `true` = probes are passing (or the host has no
    /// declared probes / mode is permissive / disabled — see
    /// `nixfleet_proto::agent_wire::host_probes_passing`). `false` = at
    /// least one probe is failing or hasn't run yet under enforce mode;
    /// the soak gate holds the Healthy → Soaked transition for this
    /// host. Hosts absent from the map (no checkin yet, or the CP
    /// projector didn't populate them) default to `true` — the gate
    /// fails open so a misconfigured projection can't accidentally
    /// stall every promotion.
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
    /// Disruption-budget snapshot copied from the rollout's signed
    /// manifest at projection time. Frozen for the rollout's life so
    /// mid-rollout retag does not reshape budget enforcement. Cross-
    /// rollout in-flight summing matches by `selector` equality — the
    /// fleet-wide property is preserved even though each rollout
    /// carries its own snapshot.
    #[serde(default)]
    pub budgets: Vec<nixfleet_proto::RolloutBudget>,
    /// `Some(t)` after `Action::ConvergeRollout` stamped the rollouts
    /// table, OR after the orphan sweep retired a rollout whose
    /// channel has no expected hosts. `None` while the rollout is
    /// still progressing through waves.
    ///
    /// Visible to the reconciler so `advance_rollout` can short-
    /// circuit terminal rollouts (no actions, no every-tick
    /// re-emission of `ConvergeRollout`). Visible to gates so
    /// `channel_edges` can read the host_states (all
    /// terminal-for-ordering by construction at this point) and
    /// release the successor — the symmetric "predecessor done"
    /// answer in both conservative + non-conservative modes.
    #[serde(default)]
    pub terminal_at: Option<DateTime<Utc>>,
}

impl Rollout {
    /// Active-for-ordering: the rollout still has work outstanding from
    /// the perspective of `channelEdges` / host-edges sequencing. Empty
    /// `host_states` (newly-recorded, no dispatches yet) counts as
    /// active — the rollout has work to do, just hasn't started.
    /// Otherwise: active iff at least one host is non-terminal.
    ///
    /// `Failed` / `Reverted` count as active (predecessor in trouble).
    /// See `HostRolloutState::is_terminal_for_ordering` for the per-host
    /// terminal predicate.
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
