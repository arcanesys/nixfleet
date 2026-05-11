# Commercial extensions - what the open kernel intentionally doesn't ship

**Status:** living document. **Owner:** open. **Last updated:** 2026-04-28.

## Why this document exists

nixfleet is a kernel framework, not an operations platform. The "stranger fleet test" is the boundary discipline: a fleet you've never seen, run by different operators with different opinions, must be able to use nixfleet without inheriting any abstracts33d-specific assumption. The framework provides options; the consuming fleet provides values.

That discipline cuts in two directions. *Inward* - keep operator-opinion code out of the kernel - is the rule that drove the v0.2 architecture refactor (kernel-only `nixfleet` + opinionated `fleet` repo). *Outward* - keep operations-platform features that real fleets eventually want from being silently absorbed into the kernel - is what this document is for.

The capabilities catalogued below are **deliberately out of scope for the open kernel**. They are not bugs, not deferred work, and not implicitly promised. A consuming fleet that needs any of them is expected to provide it via a wrapper, an external tool, or a commercial extension that lives outside this repository.

## Soft-state recovery vs hard-state recovery

ARCHITECTURE.md §8 done-criterion #1 - *"destroying the control plane's database and rebuilding from empty state results in full fleet visibility within one reconcile cycle"* - is the load-bearing claim. Phase 10 teardown work (#14) closes this claim's strict reading by classifying every CP-resident table as either:

- **Soft state** - recoverable from agent inputs (next checkin cycle) or acceptable as a one-window operational regression. Examples: `pending_confirms`, `host_rollout_state`, `token_replay`.
- **Hard state** - must come from signed artifacts pre-existing in git or trust roots. Examples: `cert_revocations` (signed `revocations.json` sidecar - #48); `trust.json` already today.

Both classes are recoverable inside the open kernel. What is NOT recoverable inside the open kernel - and what the catalogue below addresses - is **operations-grade availability and observability** of the CP itself.

## Out-of-scope capabilities

### CP high availability (multi-CP replication)

Running two or more CP instances behind a load balancer with synchronous state replication, automatic failover, leader election, and split-brain protection.

**Why out of scope:** the v0.2 design treats CP destruction as an outage, not a breach. "Outage budget" is an operator-policy decision, not a kernel concern. Implementing HA inside the kernel would require committing to a specific replication topology (raft, postgres streaming, etc.) and bind the framework to that opinion forever.

**Integration path for a fleet that needs it:** wrap the CP in a service mesh that implements its own HA (e.g. Kubernetes StatefulSet with persistent volume + standby replicas; pacemaker; provider-specific managed instances).

### Real-time signed-state snapshots to git

Pushing a signed snapshot of the CP's full operational state (rollout history, host transitions, audit trail) to a git artifact every N seconds, so that a CP rebuild restores second-by-second history rather than reconstructing from agent checkins.

**Why out of scope:** signing throughput, git churn, and operator-side complexity (key management for the snapshot key) all scale poorly. The kernel's recovery story - "agents repopulate on next checkin" - is intentionally simple. Audit-grade history with second-level granularity is a different product.

**Integration path:** sidecar daemon that subscribes to CP journal output (which already emits structured events for every state transition per RFC-0002 §7) and pushes signed batches to a separate git repo or to an audit-log service.

### SLA-grade observability of recovery cycles

Dashboards, alerting, and SLI/SLO tracking for "how long did the CP take to recover full fleet visibility after a rebuild" - measured continuously, with paging on regressions.

**Why out of scope:** this is operations-grade dashboarding. It bundles vendor-specific alerting tools (PagerDuty, Opsgenie, Slack), opinionated SLI definitions, and dashboard layouts. None of that belongs in a kernel.

**Integration path:** ingest CP journal output (`journalctl -u nixfleet-control-plane`) into the operator's existing observability stack. The kernel emits enough structured events; building dashboards on top is the consuming fleet's job.

### Audit-ready compliance packages

Pre-packaged auditor-facing reports correlating fleet state to specific compliance frameworks (SOC 2, ISO 27001, HIPAA, etc.), with evidence chains, sign-off workflows, and exportable PDFs.

**Why out of scope:** compliance is a moving target tied to specific regulatory regimes, auditor relationships, and customer contracts. The kernel ships the *primitives* - signed probes, declared compliance controls, signed `fleet.resolved` artifacts - that any compliance package can build on. Bundling specific frameworks would either cargo-cult one regime's vocabulary into every fleet, or balloon into a permanent maintenance burden tracking regulatory drift.

**Integration path:** external compliance toolchain ingests the kernel's signed artifacts + probe outputs; correlates against the framework-of-record. The kernel guarantees the artifacts are cryptographically verifiable; correlating them to specific regulatory clauses is the auditor's domain.

### Hosted CP / managed-service deployment

Running the CP as a service for fleets that don't want to host it themselves - including secret-key custody, backup management, monitoring, and 24/7 operations response.

**Why out of scope:** the entire v0.2 inversion-of-trust principle is *"the operator's organisation holds the keys."* A hosted CP that holds an operator's signing keys would dilute the principle into theatre. Even a hosted CP that holds *no* keys (operator-side signing, hosted relay only) would still bundle operations services that a kernel framework has no business shipping.

**Integration path:** a service provider that operates a fleet on the customer's behalf is a separate business. The kernel runs equally well on a self-hosted M70q or in a provider's datacentre - that's the design property.

### Multi-tenant federation

A single CP serving multiple administrative domains (different signing roots, different agent populations, different `fleet.resolved` artifacts) with tenant isolation, per-tenant quotas, and cross-tenant policy.

**Why out of scope:** ARCHITECTURE.md §7 already names this as a non-goal - *"the control plane assumes a single administrative domain."* Multi-tenancy fundamentally conflicts with the inversion-of-trust principle (each tenant's signing root is independent), and a multi-tenant CP would need to bridge those trust boundaries somehow. That bridge is exactly the kind of implicit-trust hop v0.2 was designed to remove.

**Integration path:** run one CP per tenant. Federation, if needed, happens above - at the git-forge layer (which already supports per-org repos), not at the CP layer.

### Per-host fine-grained RBAC + access control

Beyond the cert-CN-bound mTLS that today's CP enforces (every agent endpoint matches its own host's CN; operator endpoints require a specific operator cert), no per-host access control matrix is provided. There is no concept of "this operator can view but not edit", "this auditor can read history but not query secrets", or "this CI agent can fetch closures for hostnames matching a glob".

**Why out of scope:** RBAC vocabularies are the most opinion-loaded layer of any orchestration system. RFC-0001 §3 (selector algebra) covers what hosts a *rollout* applies to; that's enough vocabulary for the kernel's deployment job. Operator/auditor/CI-agent access patterns are organisation-shaped concerns.

**Integration path:** front the CP's HTTP surface with a service that does coarse-grained access control (Tailscale ACLs, Caddy with mTLS gating, auth proxy in front of `/v1/operator/*`). The kernel's mTLS-CN guarantee is the floor; finer-grained policy belongs above.

### Long-running historical metrics + capacity planning

Multi-month rollout-history retention, fleet-shape graphs over time, capacity planning suggestions ("you're scaling toward N hosts at rate R, here's what to provision").

**Why out of scope:** the CP's SQLite is sized for steady-state operations, not for being a historical metrics warehouse. Per ARCHITECTURE.md §3, observability is via structured journal lines that are queryable per-rollout - not via a persistent metrics database inside the CP.

**Integration path:** ingest CP journal output into a metrics platform (Prometheus + Mimir; Loki; Grafana stack; provider-specific ingest). The kernel's job is to *emit* the data; storing and analysing it long-term is the consuming fleet's choice of stack.

## What this means in practice

A fleet adopting nixfleet should expect:

- **The kernel works at single-CP / single-domain scale**, with clear primitives (signed artifacts, mTLS endpoints, per-host state machines, structured journal output) that higher layers can build on.
- **Recovery from CP destruction is correct but not luxurious.** Within one reconcile cycle the fleet's desired state reconverges; per-table soft-state regressions are documented and bounded.
- **Anything resembling "platform features" - HA, dashboards, audit packages, multi-tenancy - is the consuming fleet's responsibility** to source from elsewhere. The kernel intentionally does not pick those vendors.

This separation is the same discipline that separates `nixfleet` (kernel) from `fleet` (the abstracts33d deployment) - kept honest by the stranger-fleet test on every commit.

## Adding to this list

When a feature request arrives that would obviously belong in a commercial layer, file it as an issue with the `out-of-scope` label and reference this document. Don't merge a half-implementation into the kernel.

When a feature request is *borderline* - e.g. a small piece of operator UX that arguably belongs in the kernel - apply the stranger-fleet test honestly. If a fleet with different operations practices, different observability stacks, and different compliance posture wouldn't want this exact behaviour, it doesn't belong here.
