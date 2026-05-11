# nixfleet-reconciler

**Role.** Pure-function rollout reconciler and sidecar verification layer. Stateless: takes `(FleetResolved, Observed, now)` and returns a deterministic list of `Action`s. The control plane wraps it in an Axum server and a SQLite-backed `Observed`; the agent uses the verify half independently. Carries the "given fleet intent X and observed state Y, exactly what must happen next?" decision contract. No I/O, no clock except the `now` argument, no randomness - which means every action is auditable from inputs alone.

**Key types and state machines.** `Action` (the enum of decisions: `OpenRollout`, `Dispatch`, `Skip`, `Promote`, `RotateTrustRoot`, ...), `HostRolloutState` (per-host lifecycle: Queued -> Dispatched -> Activating -> Soaking -> Soaked / Failed / Reverted), `RolloutState` (per-rollout lifecycle including paused / completed), `Observed` (snapshot from the CP: channel refs, host state, active rollouts, deferrals, host-probes), `Rollout` (one open rollout's view). `verify::SignedSidecar` and `verify::VerifyError` cover signature failure modes that the auditor binary reports verbatim.

**Surface.** Library only, no binary. Public entry points: `reconcile(&FleetResolved, &Observed, now) -> Vec<Action>` (the canonical decision procedure - RFC-0002), `topological_channel_order(...)`, `verify_artifact / verify_rollout_manifest / verify_revocations / verify_signed_sidecar` (signature verification with freshness-window enforcement - RFC-0006), `compute_canonical_hash`, `compute_rollout_id`, `rollout_id_from_bytes` (canonical-bytes-in, hex-string-out; reused by the auditor so older verifiers can still validate manifests under additive evolution), `check_trust_rotations` (emits `RotateTrustRoot` actions when a slot's `retire_at` has passed and a successor exists), `project_manifest` (FleetResolved -> per-channel RolloutManifest projection).

**Links.**

- Generated rustdoc: [`api/nixfleet_reconciler/`](../../mdbook/src/api.md)
- Relevant RFCs: [RFC-0002](../../rfcs/0002-reconciler.md), [RFC-0005](../../rfcs/0005-trust-lifecycle.md), [RFC-0006](../../rfcs/0006-freshness-window-policy.md)
- Architecture component: [§1.4 Control plane](../../design/architecture.md#14-control-plane-the-router), [§3 The main flow](../../design/architecture.md#3-the-main-flow), [§4 The trust flow](../../design/architecture.md#4-the-trust-flow)
