# RFC-0001: Declarative fleet topology (`fleet.nix`)

**Status.** Draft.
**Target.** `abstracts33d/nixfleet` issue #1.
**Scope.** Schema and evaluation contract for the `fleet` flake output. Does not cover reconciliation semantics (that's #3) or activation (that's #2).

## 1. Motivation

Every seam in nixfleet today routes around a missing object: "the fleet as declared". The control plane has desired state in SQLite; the CLI has flags; the operator has intent in their head. None of these are git-tracked, reviewable, or composable. Before any of the spine items above #1 can land, we need one thing: **a pure, evaluable Nix value representing the fleet**. Everything downstream consumes it.

Design goals, in order:

1. **Pure.** `nix eval .#fleet` returns the full value with no IO, no network, no control-plane call.
2. **Self-contained.** No cross-referencing outside the flake — hosts, tags, policies all resolved at eval time.
3. **Typed.** Module system with option types; misuse fails at `nix flake check`.
4. **Composable.** A `fleet` is a value; multiple flakes can merge fleets (for org-wide super-fleets).
5. **Minimal.** Schema covers what's needed for #2/#3/#4; resists feature creep.

## 2. Schema

```nix
# flake.nix
outputs = { self, nixpkgs, nixfleet, ... }: {
  fleet = nixfleet.lib.mkFleet {
    # ------------------------------------------------------------
    # 2.1 Hosts — the atomic unit.
    # ------------------------------------------------------------
    hosts.m70q-attic = {
      system = "x86_64-linux";
      configuration = self.nixosConfigurations.m70q-attic;
      tags = [ "homelab" "always-on" "eu-fr" "server" ];
      channel = "stable";
    };

    hosts.rpi-sensor-01 = {
      system = "aarch64-linux";
      configuration = self.nixosConfigurations.rpi-sensor-01;
      tags = [ "edge" "eu-fr" ];
      channel = "edge-slow";
    };

    # ------------------------------------------------------------
    # 2.2 Tags — logical groupings, purely descriptive.
    # Tags have no hierarchy; use as many as needed per host.
    # ------------------------------------------------------------
    tags = {
      homelab.description    = "Manuel's personal fleet.";
      "always-on".description = "Expected to be reachable 24/7.";
      "eu-fr".description     = "Hosted in France; ANSSI policies apply.";
    };

    # ------------------------------------------------------------
    # 2.3 Channels — release trains.
    # Pinned to a git ref at reconcile time (see issue #3).
    # ------------------------------------------------------------
    channels.stable = {
      description = "Main production channel.";
      rolloutPolicy = "canary-conservative";
      compliance = {
        strict = true;
        frameworks = [ "anssi-bp028" ];
      };
    };
    channels.edge-slow = {
      description = "Battery-powered edge nodes; weekly reconcile.";
      rolloutPolicy = "all-at-once";
      reconcileIntervalMinutes = 10080;  # 7 days
    };

    # ------------------------------------------------------------
    # 2.4 Rollout policies — named, reusable.
    # ------------------------------------------------------------
    rolloutPolicies.canary-conservative = {
      strategy = "canary";
      waves = [
        { selector = { tags = [ "canary" ]; }; soakMinutes = 30; }
        { selector = { tagsAny = [ "non-critical" ]; }; soakMinutes = 60; }
        { selector = { all = true; }; soakMinutes = 0; }
      ];
      healthGate = {
        systemdFailedUnits.max = 0;
        complianceProbes.required = true;
      };
      onHealthFailure = "rollback-and-halt";
    };

    rolloutPolicies.all-at-once = {
      strategy = "all-at-once";
      healthGate.systemdFailedUnits.max = 0;
    };

    # ------------------------------------------------------------
    # 2.5 Edges — ordering constraints across hosts.
    # ------------------------------------------------------------
    edges = [
      { after = "db-primary"; before = "app-*"; reason = "schema migrations"; }
    ];

    # ------------------------------------------------------------
    # 2.6 Disruption budgets — max in-flight per selector.
    # ------------------------------------------------------------
    disruptionBudgets = [
      { selector = { tags = [ "etcd" ]; }; maxInFlight = 1; }
      { selector = { tags = [ "always-on" ]; }; maxInFlightPct = 50; }
    ];
  };
};
```

## 3. Selector algebra

Used by waves, edges, and budgets. Keep it minimal — resist reinventing Kubernetes label selectors.

```
selector :=
  | { tags     = [ "a" "b" ];   }   # host has ALL listed tags
  | { tagsAny  = [ "a" "b" ];   }   # host has ANY listed tag
  | { hosts    = [ "m70q" ];    }   # explicit host list
  | { channel  = "stable";      }   # all hosts on this channel
  | { all      = true;          }   # every host in the fleet
  | { not      = <selector>;    }   # negation
  | { and      = [ <sel> <sel> ]; } # intersection
```

No wildcards in host names (resolve to explicit list). No regex. Evaluates to a concrete set of hosts at flake-eval time — fully static.

## 4. Evaluation contract

### 4.1 What the control plane consumes

The control plane never evaluates Nix. It reads the resolved fleet from a single JSON artifact produced by CI:

```bash
nix eval --json .#fleet.resolved > fleet.json
```

`fleet.resolved` is a derived attribute: the above schema, but with all selectors pre-resolved to host lists, all policies inlined, and closure hashes computed per host per channel-ref pair. This is the reconciler's input.

Shape:

```json
{
  "schemaVersion": 1,
  "hosts": {
    "m70q-attic": {
      "system": "x86_64-linux",
      "closureHash": "sha256-...",
      "tags": ["homelab", "always-on", "eu-fr", "server"],
      "channel": "stable"
    }
  },
  "channels": { "stable": { "rolloutPolicy": {...}, "compliance": {...} } },
  "waves": {
    "stable": [
      { "hosts": ["canary-box"], "soakMinutes": 30 },
      { "hosts": ["rpi-01", "rpi-02"], "soakMinutes": 60 },
      { "hosts": ["m70q-attic"], "soakMinutes": 0 }
    ]
  },
  "disruptionBudgets": [
    { "hosts": ["etcd-1", "etcd-2", "etcd-3"], "maxInFlight": 1 }
  ]
}
```

### 4.2 Invariants checked at `nix flake check`

- Every host's `configuration` is a valid `nixosConfiguration`.
- Every host's `channel` exists in `channels`.
- Every channel's `rolloutPolicy` exists in `rolloutPolicies`.
- Every selector resolves to at least one host (warn, not fail — empty selectors are sometimes intentional).
- `compliance.frameworks` reference known frameworks from `nixfleet-compliance`.
- Edges form a DAG (no cycles).
- Disruption budgets are satisfiable given fleet size (warn if `maxInFlight = 1` on a 100-host budget will take forever).

### 4.3 Signed artifact contract

`fleet.resolved.json` is a trust-boundary artifact (see ARCHITECTURE.md §4). CI produces and signs it with the CI release key; every consumer verifies before use.

- **Signing.** CI writes `fleet.resolved.json` + `fleet.resolved.sig` to the channel's storage. The signature covers the full canonicalized JSON plus a `signedAt` RFC 3339 timestamp (embedded as `meta.signedAt` in the artifact).
- **Verification — control plane.** On every fetch, verifies the signature against the pinned CI release public key. Signature mismatch or unknown key → refuse to reconcile the channel; emit an alert.
- **Verification — agents (optional path).** An agent that fetches `fleet.resolved` directly (rather than receiving targets from the control plane) performs the same verification. Enables the trust-minimized bootstrap in RFC-0003 §4.
- **Key pinning.** The CI release public key is committed to the flake (`nixfleet.trust.ciReleaseKey`) and embedded in every built closure. Key rotation is a new commit + a grace window during which both keys verify.
- **Freshness.** Downstream consumers (RFC-0003 §7) enforce `now − meta.signedAt ≤ channel.freshnessWindow` to defend against stale-closure replay by a compromised control plane.

Canonicalization uses a stable, spec-defined encoding (JCS or deterministic CBOR — final choice tracked as an open question below) so that signatures produced by Nix evaluation are byte-identical to what verifiers reconstruct.

## 5. Composition

Two flakes can merge fleets:

```nix
fleet = nixfleet.lib.mergeFleets [
  (import ./fleet-paris.nix)
  (import ./fleet-lyon.nix)
];
```

Conflicts (same host name, same channel definition with different values) fail eval. Merge is associative but not commutative when policies define overrides — document the precedence (later wins).

## 6. What's deliberately out of scope

- **Secrets.** Declared alongside, not inside, the fleet schema. See #6.
- **Enrollment / host identity.** A host *exists* in the fleet schema regardless of whether it's enrolled. Enrollment is an orthogonal state (see #9).
- **Runtime state.** `fleet.resolved` is purely declarative. Observed state (which host is online, what gen is running) lives in the control plane only.
- **Dynamic host sets.** No "autoscaling" — every host is named in the flake. If you need dynamic, generate the flake from a higher-level tool.

## 7. Open questions

1. **Cross-host references.** Should an edge be able to say `{ after = <selector>; before = <selector>; }`, or only named hosts? Pro: expressive. Con: cycle detection across selectors is O(n²) and harder to reason about. Lean: named-host only in v1; revisit.
2. **Per-host policy overrides.** Should a host be able to override its channel's rollout policy for itself? Pro: one-off quirky hosts. Con: erodes the channel abstraction. Lean: no in v1; force a new channel if you need it.
3. **Schema version negotiation.** If a control plane is older than the fleet's `schemaVersion`, should it refuse or degrade? Lean: refuse, log the exact incompatibility, operator upgrades one side.
4. **Canonicalization format for `fleet.resolved.json`.** JCS (RFC 8785) is JSON-native but finicky around numbers; deterministic CBOR is stricter and smaller. Lean: JCS in v1 (debuggability wins over wire size at this fleet scale); revisit if signature drift becomes a practical issue.

## 8. Migration path from current state

- Phase 1: ship `mkFleet` alongside existing CLI. `fleet.nix` is optional; CLI still works imperatively.
- Phase 2: control plane learns to ingest `fleet.resolved`. Operators can opt in per channel.
- Phase 3: CLI `deploy` becomes syntactic sugar over channel-pointer updates. Remains for CI convenience.
- Phase 4: SQLite desired-state becomes a cache of `fleet.resolved`, not a source of truth.

---

**Next.** Natural follow-ups are RFC-0002 (rollout execution engine, consumes `fleet.resolved`, emits wave-by-wave reconciliation) and RFC-0003 (agent ↔ control plane protocol). Both are downstream of this being solid.
