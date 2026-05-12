# nixfleet-proto

**Role.** Single canonical source of every wire-format type crossing the agent / CP / CI boundary. Sits at the bottom of the workspace dependency graph; every other crate depends on it. Each `pub` module mirrors a JSON artefact on disk (fleet.resolved, revocations.json, trust.json, rollout manifest, host rollout-state marker) or an HTTP request/response body. Optional fields serialise as `null` (not omitted) so JCS bytes round-trip identically with the Nix evaluator output.

**Key types and state machines.** Per module: `fleet_resolved::FleetResolved` (the signed CI artefact projected from `mkFleet`, with `Channel`, `Host`, `RolloutPolicy`, `Wave`, `Edge`, `DisruptionBudget`, `Pin`, and `Meta` sub-types), `trust::TrustConfig` / `TrustedPubkey` / `KeySlot` (typed trust attrset deserialised from trust.json with time-aware key rotation), `revocations::Revocations` + `RevocationEntry` (signed cert-revocation sidecar), `host_rollout_state::HostRolloutState` (the per-host soak / promotion / failure state machine), `rollout_manifest::RolloutManifest` (per-rollout snapshot with `HostWave` and `RolloutBudget`), `agent_wire` and `enroll_wire` (HTTP bodies for `/v1/agent/*` and `/v1/enroll`), `compliance::ComplianceControl` (typed control with evaluate / probe projections), `fleet_view::HostsResponse` / `RolloutTrace` (operator-facing read views).

**Surface.** All types are `Serialize + Deserialize + Clone + PartialEq + Debug`. Tests round-trip every wire shape via `crates/nixfleet-proto/src/testing.rs` (gated behind the `testing` feature). Schema versioning lives on the wire types themselves (`schema_version` field where applicable); RFC-0003 owns the protocol-version policy. The crate exposes no functions and no async surface - it is purely the types.

**Links.**

- Generated rustdoc: [`api/nixfleet_proto/`](../../api/nixfleet_proto/index.html)
- Relevant RFCs: [RFC-0001](../../rfcs/0001-fleet-nix.md), [RFC-0003](../../rfcs/0003-protocol.md), [RFC-0005](../../rfcs/0005-trust-lifecycle.md)
- Architecture component: [§1.2 CI](../../design/architecture.md#12-continuous-integration-the-intent-signing-oracle), [§1.4 Control plane](../../design/architecture.md#14-control-plane-the-router), [§1.5 Agent](../../design/architecture.md#15-agent-the-actuator)
