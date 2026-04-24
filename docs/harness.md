# Fleet simulation harness

A `microvm.nix`-based substrate for booting one control-plane VM and N
agent VMs cheaply, so fleet-scale scenarios (rollout, rollback, health
gates, compliance gates) can run under `nix flake check` without the
cost of full-QEMU `nixosTest` nodes.

Tracked by issue [#5](https://github.com/abstracts33d/nixfleet/issues/5).

This harness is independent from the v0.1 fleet scenarios under
`modules/tests/_vm-fleet-scenarios/`. Those run full-closure nixosTest
nodes against the v0.1 `services.nixfleet-agent` / `services.nixfleet-control-plane`.
The harness here runs lightweight microvms against stub CP/agent units
while the v0.2 skeletons are still being built; the substrate will host
the real binaries once they exist.

## Layout

```
tests/harness/
  default.nix              # entry point, exposes scenarios as a flake check attrset
  lib.nix                  # mkCpNode / mkAgentNode / mkFleetScenario / mkHarnessCerts
  nodes/
    cp.nix                 # stub CP microvm (socat + static fleet.resolved.json over mTLS)
    agent.nix              # stub agent microvm (curl + jq, emits harness-agent-ok marker)
  scenarios/
    smoke.nix              # 1 CP + 2 agents, asserts both fetches succeed within 60s
  fixtures/
    fleet-resolved.json    # hand-simplified v1 resolved artifact (2 hosts, 1 channel)
modules/tests/harness.nix  # flake-parts module that registers the checks
```

## Running

Scenarios are registered under `checks.x86_64-linux.fleet-harness-*`.

```bash
# Build just the smoke scenario (doesn't run yet - user invokes manually):
nix build .#checks.x86_64-linux.fleet-harness-smoke --no-link

# List every harness scenario currently registered:
nix eval .#checks.x86_64-linux --apply \
  'cs: builtins.filter (n: builtins.match "fleet-harness-.*" n != null) (builtins.attrNames cs)'
```

`nix flake check` runs every harness scenario alongside the eval tests
and v0.1 scenarios. Expect ≤5 min for the smoke scenario on a 16GB
machine once everything builds.

## Adding a scenario

Scenarios are standalone so one failure does not mask the others (same
convention as `modules/tests/_vm-fleet-scenarios`).

1. Copy `tests/harness/scenarios/smoke.nix` to a new file, e.g.
   `freshness-refusal.nix`.
2. Change the `name` passed to `mkFleetScenario` (this becomes the
   check attribute name).
3. Flip the scenario-specific bits: inject a tampered fixture, change
   the agent config, add guest-to-guest networking, etc.
4. Import it from `tests/harness/default.nix` with a descriptive key
   (e.g. `fleet-harness-freshness-refusal`).
5. If new hostnames are needed, add them to the `mkHarnessCerts` call
   in `default.nix` so their client certs exist.

### Scaling to `fleet-N`

The issue-#5 acceptance target is `checks.<system>.fleet-N` for
arbitrary N. The `mkFleetScenario` API already supports this: `nodes`
is an attrset indexed by hostname, and `smoke.nix` generates its agent
list with `map`. To ship `fleet-5`:

```nix
let agentNames = map (i: "agent-${toString i}") (lib.range 1 5); in ...
```

Each extra agent adds one microvm guest under the host VM. With the
default 256 MB per guest, a 16GB machine can host fleet-20 comfortably
(budget from the issue: ≤512 MB/VM).

## Known limitations (scaffold scope)

- **Stub CP**: `nodes/cp.nix` serves a static `fleet.resolved.json`
  over mTLS via `socat`. It does not yet use `services.nixfleet-control-plane`.
  Swap once the v0.2 CP skeleton lands.
- **Stub agent**: `nodes/agent.nix` is a oneshot curl + jq that emits
  `harness-agent-ok`. It does not yet use `services.nixfleet-agent` or
  invoke Stream C's p256 verify path. Swap once the v0.2 agent skeleton
  and signature-verify code land.
- **No signature verification yet**: the fixture has
  `meta.signatureAlgorithm: null`. Once Stream B signs artifacts and
  Stream C verifies them, the agent stub becomes the wire-up point —
  search for `TODO(5)` markers in `nodes/agent.nix`.
- **Only `fleet-harness-smoke` is registered**: no canary, no rollback,
  no compliance, no freshness scenarios yet. Those are Checkpoint 2
  work gated on v0.2 skeletons.
- **x86_64-linux only**: microvm.nix's host KVM path is Linux-only.
  Darwin hosts cannot run the harness directly.

## Where this plugs into the v0.2 cycle

Stream A signs `fleet.resolved.json` in lab CI; Stream C's reconciler
verifies the signature. The first real Checkpoint 2 end-to-end test
looks like:

1. Stream A's sign job writes a signed artifact matching the shape in
   `tests/harness/fixtures/fleet-resolved.json`.
2. Stream C's agent skeleton replaces `tests/harness/nodes/agent.nix`'s
   curl + jq with a real reconciler that calls the p256 verify path.
3. The `fleet-harness-smoke` scenario asserts the agent accepts the
   signed artifact; a twin scenario asserts it refuses a tampered one.

Every `TODO(5):` comment in the scaffold points at one of those slot-in
points.
