//! Fluent FleetResolved builder for tests across reconciler/CP/release.
//!
//! Defaults: `system = "x86_64-linux"`, `closure_hash = Some("{host}-closure")`,
//! channel `compliance.mode = "disabled"`, policy strategy `all-at-once` with one
//! `Selector::default` wave at 5 min soak, `OnHealthFailure::Halt`. Override via
//! `host_*` / `channel_*` / `policy_*` setters.

use std::collections::HashMap;

use crate::{
    Channel, ChannelEdge, Compliance, DisruptionBudget, Edge, FleetResolved, HealthGate, Host,
    Meta, OnHealthFailure, PolicyWave, RolloutPolicy, Selector, Wave,
};

const DEFAULT_POLICY: &str = "p";
const DEFAULT_SYSTEM: &str = "x86_64-linux";
const DEFAULT_SOAK_MINUTES: u32 = 5;

pub struct FleetBuilder {
    hosts: HashMap<String, Host>,
    channels: HashMap<String, Channel>,
    policies: HashMap<String, RolloutPolicy>,
    waves: HashMap<String, Vec<Wave>>,
    edges: Vec<Edge>,
    channel_edges: Vec<ChannelEdge>,
    budgets: Vec<DisruptionBudget>,
    meta: Meta,
}

impl Default for FleetBuilder {
    fn default() -> Self {
        Self {
            hosts: HashMap::new(),
            channels: HashMap::new(),
            policies: HashMap::new(),
            waves: HashMap::new(),
            edges: Vec::new(),
            channel_edges: Vec::new(),
            budgets: Vec::new(),
            meta: Meta {
                schema_version: 1,
                signed_at: None,
                ci_commit: None,
                signature_algorithm: Some("ed25519".into()),
            },
        }
    }
}

impl FleetBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a host on `channel` with default system + `{name}-closure`.
    /// Auto-creates the channel and the default policy if missing.
    pub fn host(mut self, name: &str, channel: &str) -> Self {
        self.ensure_channel(channel);
        self.hosts.insert(
            name.to_string(),
            Host {
                system: DEFAULT_SYSTEM.into(),
                tags: Vec::new(),
                channel: channel.to_string(),
                closure_hash: Some(format!("{name}-closure")),
                pubkey: None,
                pin: None,
            },
        );
        self
    }

    pub fn host_system(mut self, name: &str, system: &str) -> Self {
        let h = self
            .hosts
            .get_mut(name)
            .unwrap_or_else(|| panic!("FleetBuilder.host_system: unknown host {name}"));
        h.system = system.to_string();
        self
    }

    pub fn host_tag(mut self, name: &str, tag: &str) -> Self {
        let h = self
            .hosts
            .get_mut(name)
            .unwrap_or_else(|| panic!("FleetBuilder.host_tag: unknown host {name}"));
        h.tags.push(tag.to_string());
        self
    }

    pub fn host_closure(mut self, name: &str, closure: &str) -> Self {
        let h = self
            .hosts
            .get_mut(name)
            .unwrap_or_else(|| panic!("FleetBuilder.host_closure: unknown host {name}"));
        h.closure_hash = Some(closure.to_string());
        self
    }

    /// Strip the closure_hash (host present but unscheduled).
    pub fn host_no_closure(mut self, name: &str) -> Self {
        let h = self
            .hosts
            .get_mut(name)
            .unwrap_or_else(|| panic!("FleetBuilder.host_no_closure: unknown host {name}"));
        h.closure_hash = None;
        self
    }

    pub fn host_pubkey(mut self, name: &str, pubkey: &str) -> Self {
        let h = self
            .hosts
            .get_mut(name)
            .unwrap_or_else(|| panic!("FleetBuilder.host_pubkey: unknown host {name}"));
        h.pubkey = Some(pubkey.to_string());
        self
    }

    /// Idempotent: re-declaring a channel keeps the existing definition.
    pub fn channel(mut self, name: &str, policy: &str) -> Self {
        self.ensure_policy(policy);
        self.channels
            .entry(name.to_string())
            .or_insert_with(|| default_channel(policy));
        self
    }

    pub fn channel_compliance(mut self, name: &str, mode: &str, frameworks: &[&str]) -> Self {
        let c = self
            .channels
            .get_mut(name)
            .unwrap_or_else(|| panic!("FleetBuilder.channel_compliance: unknown channel {name}"));
        c.compliance = Compliance {
            mode: mode.to_string(),
            frameworks: frameworks.iter().map(|s| s.to_string()).collect(),
        };
        self
    }

    /// Idempotent: re-declaring a policy keeps the existing definition.
    pub fn policy(mut self, name: &str) -> Self {
        self.ensure_policy(name);
        self
    }

    pub fn policy_strategy(mut self, name: &str, strategy: &str) -> Self {
        self.ensure_policy(name);
        self.policies.get_mut(name).unwrap().strategy = strategy.to_string();
        self
    }

    /// Replaces the policy's wave list with one wave: `selector + soak_minutes`.
    /// Use `policy_waves` for multiple.
    pub fn policy_wave(mut self, name: &str, selector: Selector, soak_minutes: u32) -> Self {
        self.ensure_policy(name);
        self.policies.get_mut(name).unwrap().waves = vec![PolicyWave {
            selector,
            soak_minutes,
        }];
        self
    }

    pub fn policy_waves(mut self, name: &str, waves: Vec<PolicyWave>) -> Self {
        self.ensure_policy(name);
        self.policies.get_mut(name).unwrap().waves = waves;
        self
    }

    pub fn policy_on_failure(mut self, name: &str, on_health_failure: OnHealthFailure) -> Self {
        self.ensure_policy(name);
        self.policies.get_mut(name).unwrap().on_health_failure = on_health_failure;
        self
    }

    pub fn channel_edge(mut self, gates: &str, gated: &str) -> Self {
        self.channel_edges.push(ChannelEdge {
            gates: gates.to_string(),
            gated: gated.to_string(),
            reason: None,
        });
        self
    }

    pub fn edge(mut self, gates: &str, gated: &str) -> Self {
        self.edges.push(Edge {
            gates: gates.to_string(),
            gated: gated.to_string(),
            reason: None,
        });
        self
    }

    /// Single-wave channel plan: `hosts` in order, default soak.
    pub fn wave(self, channel: &str, hosts: &[&str]) -> Self {
        self.wave_with_soak(channel, hosts, DEFAULT_SOAK_MINUTES)
    }

    pub fn wave_with_soak(mut self, channel: &str, hosts: &[&str], soak_minutes: u32) -> Self {
        self.waves
            .entry(channel.to_string())
            .or_default()
            .push(Wave {
                hosts: hosts.iter().map(|s| s.to_string()).collect(),
                soak_minutes,
            });
        self
    }

    pub fn budget(mut self, selector: Selector, max_in_flight: Option<u32>) -> Self {
        self.budgets.push(DisruptionBudget {
            selector,
            max_in_flight,
            max_in_flight_pct: None,
        });
        self
    }

    pub fn meta(mut self, meta: Meta) -> Self {
        self.meta = meta;
        self
    }

    pub fn build(self) -> FleetResolved {
        FleetResolved {
            schema_version: 1,
            hosts: self.hosts,
            channels: self.channels,
            rollout_policies: self.policies,
            waves: self.waves,
            edges: self.edges,
            channel_edges: self.channel_edges,
            disruption_budgets: self.budgets,
            meta: self.meta,
        }
    }

    fn ensure_channel(&mut self, name: &str) {
        if !self.channels.contains_key(name) {
            self.ensure_policy(DEFAULT_POLICY);
            self.channels
                .insert(name.to_string(), default_channel(DEFAULT_POLICY));
        }
    }

    fn ensure_policy(&mut self, name: &str) {
        self.policies
            .entry(name.to_string())
            .or_insert_with(default_policy);
    }
}

fn default_channel(policy: &str) -> Channel {
    Channel {
        rollout_policy: policy.to_string(),
        reconcile_interval_minutes: 30,
        signing_interval_minutes: 60,
        freshness_window: 1440,
        compliance: Compliance {
            mode: "disabled".into(),
            frameworks: Vec::new(),
        },
    }
}

fn default_policy() -> RolloutPolicy {
    RolloutPolicy {
        strategy: "all-at-once".into(),
        waves: vec![PolicyWave {
            selector: Selector::default(),
            soak_minutes: DEFAULT_SOAK_MINUTES,
        }],
        health_gate: HealthGate::default(),
        on_health_failure: OnHealthFailure::Halt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_creates_channel_and_policy() {
        let f = FleetBuilder::new().host("host-05", "edge").build();
        assert!(f.hosts.contains_key("host-05"));
        assert!(f.channels.contains_key("edge"));
        assert!(f.rollout_policies.contains_key("p"));
        assert_eq!(
            f.hosts["host-05"].closure_hash.as_deref(),
            Some("host-05-closure")
        );
        assert_eq!(f.hosts["host-05"].system, "x86_64-linux");
    }

    #[test]
    fn channel_edge_recorded() {
        let f = FleetBuilder::new()
            .host("host-05", "edge")
            .host("host-01", "stable")
            .channel_edge("edge", "stable")
            .build();
        assert_eq!(f.channel_edges.len(), 1);
        assert_eq!(f.channel_edges[0].gates, "edge");
        assert_eq!(f.channel_edges[0].gated, "stable");
    }

    #[test]
    fn wave_appends() {
        let f = FleetBuilder::new()
            .host("a", "stable")
            .host("b", "stable")
            .wave("stable", &["a"])
            .wave("stable", &["b"])
            .build();
        assert_eq!(f.waves["stable"].len(), 2);
        assert_eq!(f.waves["stable"][0].hosts, vec!["a".to_string()]);
    }

    #[test]
    fn host_setters_mutate_existing() {
        let f = FleetBuilder::new()
            .host("host-05", "edge")
            .host_tag("host-05", "server")
            .host_no_closure("host-05")
            .build();
        assert_eq!(f.hosts["host-05"].tags, vec!["server".to_string()]);
        assert!(f.hosts["host-05"].closure_hash.is_none());
    }
}
