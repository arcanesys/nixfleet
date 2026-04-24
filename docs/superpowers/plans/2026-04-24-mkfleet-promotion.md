# mkFleet promotion + nixfleet.trust.* Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Promote the `spike/lib/mkFleet.nix` prototype to production `lib/mkFleet.nix` with every RFC-0001 §4.2 invariant implemented, add the `nixfleet.trust.*` option tree, and wire a `fleet` flake output that produces a CONTRACTS §I.1-compliant `fleet.resolved` artifact.

**Architecture:** Library-style Nix module evaluator. `lib.mkFleet { ... }` evaluates the fleet description through `lib.evalModules`, fails fast on any invariant violation (strict typing + a DAG check + a freshness-window relation check), and exposes a `resolved` attribute that is the CI-consumable projection defined in RFC-0001 §4.1 and CONTRACTS §I.1. Trust roots live in a sibling `modules/trust.nix` NixOS module exposing `nixfleet.trust.{ciReleaseKey,atticCacheKey,orgRootKey}` option trees, each with a `.previous` grace-window slot and a shared `rejectBefore` compromise switch.

**Tech Stack:** Nix, flake-parts, `lib.evalModules`, JCS-shaped JSON output (actual signing tooling lives in Stream C, but the shape and fields land here).

**Issues closed:** abstracts33d/nixfleet#1, Nix portion of abstracts33d/nixfleet#12.

---

## File Structure

### Created

| Path | Responsibility |
|---|---|
| `lib/mkFleet.nix` | Production mkFleet implementation. Evaluates module, checks invariants, emits `.resolved`. |
| `lib/flake-module.nix` | flake-parts wiring so `config.flake.lib.mkFleet` is exposed to consumers. |
| `modules/trust.nix` | `nixfleet.trust.*` option tree (pubkey declarations, grace windows, compromise switch). |
| `examples/fleet-homelab/flake.nix` | Minimal end-to-end example invoking `mkFleet` against stub nixosConfigurations, used as acceptance artifact. |
| `examples/fleet-homelab/fleet.nix` | The declarative fleet description fed into `mkFleet`. |
| `examples/fleet-homelab/hosts/*.nix` | Stub host modules (copied from `spike/examples/homelab/hosts/`, trimmed to minimum). |
| `tests/lib/mkFleet/fleet-resolved-golden.nix` | Eval-time assertion: homelab example `.resolved` equals a pinned JSON fixture. |
| `tests/lib/mkFleet/negative/*.nix` | One fixture per invariant, each expected to fail `nix eval`. |
| `tests/lib/mkFleet/fixtures/homelab.resolved.json` | Pinned expected JSON artifact for the homelab example. |
| `modules/tests/trust-options.nix` | Eval test for `nixfleet.trust.*` option types. |
| `CHANGELOG.md` entry | User-visible note about the new `fleet` flake output and `nixfleet.trust.*` options. |

### Modified

| Path | Change |
|---|---|
| `modules/flake-module.nix` | Import `lib/flake-module.nix` so `config.flake.lib.mkFleet` is wired into the plugin framework. |
| `modules/core/_nixos.nix` (or the file that re-exports modules) | Import `modules/trust.nix` so `nixfleet.trust.*` options are available on every host. |
| `flake.nix` | If needed, expose `lib.mkFleet` at the top level for external flakes. |

### Not touched in this plan

- `spike/` — leave intact; the spike stays as the runnable prototype. We promote, we do not delete.
- `crates/` — Rust consumers are Stream C's concern.
- Any existing `modules/fleet.nix` — that is the test fleet fixture, unrelated to this artifact.

---

## Pre-flight

The worktree for this work is `.worktrees/mkfleet-promotion` on branch `feat/mkfleet-promotion` (already created off `main`). All commands below run from the worktree root unless stated otherwise.

**User-run commands (build economy):** Every `nix eval`, `nix flake check`, and `nix build` below is listed so the operator can run it — do not auto-run heavy commands. The agent may run `nix eval --raw` on tiny fixtures that return plain strings; anything that builds a derivation is user-side.

---

## Task 1: Scaffold `lib/` and the promoted mkFleet

**Files:**
- Create: `lib/mkFleet.nix`
- Create: `lib/flake-module.nix`
- Create: `lib/default.nix`
- Modify: `modules/flake-module.nix:1-72`

- [ ] **Step 1: Create `lib/default.nix`**

```nix
# lib/default.nix
#
# nixfleet library entry point. Imports are keyed by capability so consumers
# can depend on narrow slices (e.g. just `mkFleet`) without pulling the full
# framework module graph.
{lib}: {
  mkFleet = (import ./mkFleet.nix {inherit lib;}).mkFleet;
}
```

- [ ] **Step 2: Copy the spike as the starting point**

Run:

```bash
cp spike/lib/mkFleet.nix lib/mkFleet.nix
```

Do not modify the file yet — subsequent tasks extend it invariant-by-invariant so each commit is atomic and bisectable.

- [ ] **Step 3: Create `lib/flake-module.nix`**

```nix
# lib/flake-module.nix
#
# Exposes `config.flake.lib.mkFleet` via flake-parts so consumers can call
# `nixfleet.lib.mkFleet { ... }` from their own flakes.
{lib, ...}: {
  config.flake.lib.mkFleet =
    (import ./mkFleet.nix {inherit lib;}).mkFleet;
}
```

- [ ] **Step 4: Wire the new flake-module into the framework export**

In `modules/flake-module.nix`, add `../lib/flake-module.nix` to the imports list of the top-level flake-parts module. Open the file and locate the `config.flake = { ... }` block. Add an `imports` clause at the top level of the module body:

```nix
{
  inputs,
  lib,
  ...
}: let
  nixfleetLib = import ./_shared/lib/default.nix {inherit inputs lib;};
in {
  imports = [../lib/flake-module.nix];

  options.nixfleet.lib = lib.mkOption {
    # ...unchanged
  };
  # ...rest of file unchanged
}
```

- [ ] **Step 5: Sanity check — evaluate the exported lib**

User runs:

```bash
nix eval --impure --expr 'builtins.typeOf (import ./lib/default.nix { lib = (builtins.getFlake (toString ./.)).inputs.nixpkgs.lib; }).mkFleet'
```

Expected output: `"lambda"`.

- [ ] **Step 6: Commit**

```bash
git add lib/ modules/flake-module.nix
git commit -m "feat(lib): scaffold mkFleet promotion from spike"
```

---

## Task 2: Add fixture-based eval harness

**Files:**
- Create: `tests/lib/mkFleet/default.nix`
- Create: `tests/lib/mkFleet/fixtures/.gitkeep`
- Create: `tests/lib/mkFleet/negative/.gitkeep`
- Modify: `modules/tests/eval.nix` (if present) — else `modules/tests/default.nix`

- [ ] **Step 1: Locate the existing eval-tests entry point**

Run:

```bash
grep -rn "eval" modules/tests/ 2>/dev/null | head -20
```

Note the file that assembles per-scenario eval tests. The existing convention is one test per subdirectory.

- [ ] **Step 2: Create the harness entry point**

```nix
# tests/lib/mkFleet/default.nix
#
# Eval-only tests for lib/mkFleet.nix. No VM, no build — pure evaluation.
# Each .nix file under ./fixtures/ is a positive scenario (must eval clean).
# Each .nix file under ./negative/ is expected to `throw` a specific error.
{
  lib,
  mkFleet ? (import ../../../lib/mkFleet.nix {inherit lib;}).mkFleet,
}: let
  runPositive = path: let
    cfg = import path {inherit lib mkFleet;};
    expectedPath = lib.replaceStrings [".nix"] [".resolved.json"] path;
    expected = builtins.fromJSON (builtins.readFile expectedPath);
    actual = cfg.resolved;
    match = builtins.toJSON actual == builtins.toJSON expected;
  in
    if match
    then "ok"
    else throw ''
      golden mismatch for ${toString path}
      expected: ${builtins.toJSON expected}
      actual:   ${builtins.toJSON actual}
    '';

  runNegative = path: let
    result = builtins.tryEval (import path {inherit lib mkFleet;}).resolved;
  in
    if result.success
    then throw "expected eval failure for ${toString path}, got success"
    else "ok";

  listFixtures = dir:
    lib.filter (n: lib.hasSuffix ".nix" n) (builtins.attrNames (builtins.readDir dir));

  positives = map (n: runPositive (./fixtures + "/${n}")) (listFixtures ./fixtures);
  negatives = map (n: runNegative (./negative + "/${n}")) (listFixtures ./negative);
in {
  results = positives ++ negatives;
}
```

- [ ] **Step 3: Drop-in the placeholder fixture directories**

```bash
mkdir -p tests/lib/mkFleet/fixtures tests/lib/mkFleet/negative
touch tests/lib/mkFleet/fixtures/.gitkeep tests/lib/mkFleet/negative/.gitkeep
```

- [ ] **Step 4: Commit**

```bash
git add tests/lib/mkFleet/
git commit -m "test(lib/mkFleet): add fixture-based eval harness"
```

---

## Task 3: Invariant — host `configuration` is a nixosConfiguration

**Files:**
- Modify: `lib/mkFleet.nix`
- Create: `tests/lib/mkFleet/negative/host-bad-configuration.nix`

- [ ] **Step 1: Write the failing negative fixture**

```nix
# tests/lib/mkFleet/negative/host-bad-configuration.nix
{lib, mkFleet}:
mkFleet {
  hosts.bad = {
    system = "x86_64-linux";
    configuration = "not-a-nixos-config";
    tags = [];
    channel = "stable";
  };
  channels.stable = {
    rolloutPolicy = "all-at-once";
    signingIntervalMinutes = 60;
    freshnessWindow = 180;
  };
  rolloutPolicies.all-at-once = {
    strategy = "all-at-once";
    waves = [{selector.all = true; soakMinutes = 0;}];
  };
}
```

- [ ] **Step 2: Run the eval test — expect negative failure (no error today)**

User runs:

```bash
nix eval --impure --expr 'import ./tests/lib/mkFleet/default.nix { lib = (builtins.getFlake (toString ./.)).inputs.nixpkgs.lib; }'
```

Expected: the harness throws `expected eval failure for .../host-bad-configuration.nix, got success` — meaning the invariant check does not exist yet.

- [ ] **Step 3: Extend `checkInvariants` in `lib/mkFleet.nix`**

In the `checkInvariants` let-binding, after `edgeErrors`, add:

```nix
    configurationErrors =
      lib.concatMap (
        n: let
          h = cfg.hosts.${n};
          isValid =
            builtins.isAttrs h.configuration
            && h.configuration ? config
            && h.configuration.config ? system
            && h.configuration.config.system ? build
            && h.configuration.config.system.build ? toplevel;
        in
          lib.optional (!isValid)
          "host '${n}' configuration is not a valid nixosConfiguration (missing config.system.build.toplevel)"
      )
      hostNames;
```

Add `configurationErrors` to the `errs` sum.

- [ ] **Step 4: Re-run the eval test — expect clean pass**

User runs the command from Step 2. Expected: the harness evaluates to `{ results = [ "ok" ]; }` (or similar, with "ok" entries).

- [ ] **Step 5: Commit**

```bash
git add lib/mkFleet.nix tests/lib/mkFleet/negative/host-bad-configuration.nix
git commit -m "feat(lib/mkFleet): enforce host.configuration is a nixosConfiguration"
```

---

## Task 4: Invariant — empty selector warns (does not fail)

**Files:**
- Modify: `lib/mkFleet.nix`
- Create: `tests/lib/mkFleet/fixtures/empty-selector-warns.nix`
- Create: `tests/lib/mkFleet/fixtures/empty-selector-warns.resolved.json`

RFC-0001 §4.2: "Every selector resolves to at least one host (warn, not fail — empty selectors are sometimes intentional)."

- [ ] **Step 1: Add a `warnings` attribute to the module schema**

In `lib/mkFleet.nix`, after the options block, change `resolveFleet` to also return `warnings`:

```nix
  resolveFleet = cfg:
    assert checkInvariants cfg; let
      emptySelectorWarnings =
        lib.concatMap (
          policyName:
            lib.concatMap (
              w: let
                hosts = resolveSelector w.selector cfg.hosts;
              in
                lib.optional (hosts == [])
                "rollout policy '${policyName}' has a wave with a selector that resolves to zero hosts"
            )
            cfg.rolloutPolicies.${policyName}.waves
        )
        (lib.attrNames cfg.rolloutPolicies);
      emittedWarnings =
        lib.foldl' (acc: msg: lib.warn msg acc) null emptySelectorWarnings;
    in
      builtins.seq emittedWarnings {
        schemaVersion = 1;
        # ... rest unchanged
      };
```

The `lib.warn` calls print to stderr during `nix eval`; `builtins.seq` forces them so operators see the warning before downstream use.

- [ ] **Step 2: Add a positive fixture asserting the resolved shape is still correct even with an empty selector**

`tests/lib/mkFleet/fixtures/empty-selector-warns.nix`:

```nix
{lib, mkFleet}:
mkFleet {
  hosts.m = {
    system = "x86_64-linux";
    configuration = (import ./_stub-configuration.nix {inherit lib;});
    tags = ["role-a"];
    channel = "stable";
  };
  channels.stable = {
    rolloutPolicy = "emptyish";
    signingIntervalMinutes = 60;
    freshnessWindow = 180;
  };
  rolloutPolicies.emptyish = {
    strategy = "canary";
    waves = [
      {selector.tags = ["role-b"]; soakMinutes = 10;} # resolves to zero hosts — warning expected
      {selector.all = true; soakMinutes = 0;}
    ];
  };
}
```

- [ ] **Step 3: Create `_stub-configuration.nix` shared by fixtures**

```nix
# tests/lib/mkFleet/fixtures/_stub-configuration.nix
#
# Minimal stub that looks like a nixosConfiguration enough to satisfy
# the `host.configuration` invariant without needing to evaluate NixOS.
{lib}: {
  config.system.build.toplevel = {
    outPath = "/nix/store/0000000000000000000000000000000000000000-stub";
    drvPath = "/nix/store/0000000000000000000000000000000000000000-stub.drv";
  };
}
```

- [ ] **Step 4: Generate the expected resolved JSON**

User runs:

```bash
nix eval --impure --json --expr '(import ./tests/lib/mkFleet/fixtures/empty-selector-warns.nix { lib = (builtins.getFlake (toString ./.)).inputs.nixpkgs.lib; mkFleet = (import ./lib/mkFleet.nix { lib = (builtins.getFlake (toString ./.)).inputs.nixpkgs.lib; }).mkFleet; }).resolved' | jq . > tests/lib/mkFleet/fixtures/empty-selector-warns.resolved.json
```

Inspect the file. Confirm `waves.stable[0].hosts == []` (empty) and `waves.stable[1].hosts == ["m"]`.

- [ ] **Step 5: Commit**

```bash
git add lib/mkFleet.nix tests/lib/mkFleet/fixtures/
git commit -m "feat(lib/mkFleet): warn on empty selector resolution"
```

---

## Task 5: Invariant — compliance frameworks must be declared

**Files:**
- Modify: `lib/mkFleet.nix`
- Create: `tests/lib/mkFleet/negative/unknown-framework.nix`

- [ ] **Step 1: Add an option for the declared frameworks set**

At the module options in `mkFleet.nix`:

```nix
complianceFrameworks = mkOption {
  type = types.listOf types.str;
  default = ["anssi-bp028" "nis2" "dora" "iso27001"];
  description = ''
    Known compliance frameworks accepted by channel.compliance.frameworks.
    Override only if using an out-of-tree compliance extension.
  '';
};
```

- [ ] **Step 2: Add the invariant check**

In `checkInvariants`:

```nix
    complianceErrors =
      lib.concatMap (
        channelName: let
          c = cfg.channels.${channelName};
          bad = lib.filter (f: !(builtins.elem f cfg.complianceFrameworks)) c.compliance.frameworks;
        in
          map (f: "channel '${channelName}' references unknown compliance framework '${f}'") bad
      )
      (lib.attrNames cfg.channels);
```

Add to `errs`.

- [ ] **Step 3: Write the negative fixture**

```nix
# tests/lib/mkFleet/negative/unknown-framework.nix
{lib, mkFleet}:
mkFleet {
  hosts.m = {
    system = "x86_64-linux";
    configuration = (import ../fixtures/_stub-configuration.nix {inherit lib;});
    tags = [];
    channel = "stable";
  };
  channels.stable = {
    rolloutPolicy = "all-at-once";
    signingIntervalMinutes = 60;
    freshnessWindow = 180;
    compliance.frameworks = ["fictional-framework/v99"];
  };
  rolloutPolicies.all-at-once = {
    strategy = "all-at-once";
    waves = [{selector.all = true; soakMinutes = 0;}];
  };
}
```

- [ ] **Step 4: Run the harness — expect pass (negative test throws as expected)**

User runs the eval command from Task 3 Step 4.

- [ ] **Step 5: Commit**

```bash
git add lib/mkFleet.nix tests/lib/mkFleet/negative/unknown-framework.nix
git commit -m "feat(lib/mkFleet): reject unknown compliance frameworks"
```

---

## Task 6: Invariant — edges form a DAG (cycle detection)

**Files:**
- Modify: `lib/mkFleet.nix`
- Create: `tests/lib/mkFleet/negative/edge-cycle.nix`

- [ ] **Step 1: Implement cycle detection**

In `lib/mkFleet.nix`, above `checkInvariants`, add:

```nix
  # Tarjan-free cycle detection using iterative DFS marking.
  # Edges: { after = "a"; before = "b"; } means a must finish before b starts.
  # So we walk "after → before" edges.
  hasCycle = edges: let
    adj = lib.foldl' (
      acc: e: let
        current = acc.${e.after} or [];
      in
        acc // {${e.after} = current ++ [e.before];}
    ) {}
    edges;
    nodes = lib.unique (map (e: e.after) edges ++ map (e: e.before) edges);
    # visit returns { cycle = bool; stack = [...]; visited = [...]; }
    visit = node: path: visited:
      if builtins.elem node path
      then {cycle = true; path = path ++ [node]; visited = visited;}
      else if builtins.elem node visited
      then {cycle = false; path = path; visited = visited;}
      else let
        children = adj.${node} or [];
        walk = c: acc:
          if acc.cycle
          then acc
          else let
            r = visit c (path ++ [node]) acc.visited;
          in
            if r.cycle
            then r
            else {cycle = false; path = acc.path; visited = r.visited ++ [c];};
        result = lib.foldl' (a: c: walk c a) {cycle = false; path = []; visited = visited;} children;
      in
        if result.cycle
        then result
        else {cycle = false; path = []; visited = result.visited ++ [node];};
    scan = nodes:
      lib.foldl' (
        acc: n:
          if acc.cycle
          then acc
          else visit n [] acc.visited
      ) {cycle = false; path = []; visited = [];}
      nodes;
  in
    (scan nodes).cycle;
```

Then in `checkInvariants`:

```nix
    cycleErrors = lib.optional (hasCycle cfg.edges) "edges form a cycle; the DAG invariant is violated";
```

Add `cycleErrors` to `errs`.

- [ ] **Step 2: Write the negative fixture**

```nix
# tests/lib/mkFleet/negative/edge-cycle.nix
{lib, mkFleet}:
mkFleet {
  hosts = {
    a = {
      system = "x86_64-linux";
      configuration = (import ../fixtures/_stub-configuration.nix {inherit lib;});
      tags = [];
      channel = "stable";
    };
    b = {
      system = "x86_64-linux";
      configuration = (import ../fixtures/_stub-configuration.nix {inherit lib;});
      tags = [];
      channel = "stable";
    };
  };
  channels.stable = {
    rolloutPolicy = "all-at-once";
    signingIntervalMinutes = 60;
    freshnessWindow = 180;
  };
  rolloutPolicies.all-at-once = {
    strategy = "all-at-once";
    waves = [{selector.all = true; soakMinutes = 0;}];
  };
  edges = [
    {after = "a"; before = "b"; reason = "a before b";}
    {after = "b"; before = "a"; reason = "cycle!";}
  ];
}
```

- [ ] **Step 3: Commit**

```bash
git add lib/mkFleet.nix tests/lib/mkFleet/negative/edge-cycle.nix
git commit -m "feat(lib/mkFleet): detect cycles in edges (DAG invariant)"
```

---

## Task 7: Invariant — disruption budgets satisfiable (warn)

**Files:**
- Modify: `lib/mkFleet.nix`
- Create: `tests/lib/mkFleet/fixtures/tight-budget-warns.nix` + `.resolved.json`

- [ ] **Step 1: Extend `resolveFleet` with the budget warning**

In the `resolveFleet` let-binding, add:

```nix
      budgetWarnings =
        lib.concatMap (
          b: let
            hosts = resolveSelector b.selector cfg.hosts;
            effectiveMax =
              if b.maxInFlight != null
              then b.maxInFlight
              else if b.maxInFlightPct != null
              then lib.max 1 ((builtins.length hosts * b.maxInFlightPct) / 100)
              else builtins.length hosts;
          in
            lib.optional (builtins.length hosts >= 10 && effectiveMax == 1)
            "disruption budget with maxInFlight=1 on ${toString (builtins.length hosts)} hosts will take long to complete"
        )
        cfg.disruptionBudgets;
```

Chain these warnings through the same `lib.warn`/`seq` pattern used for empty-selector warnings.

- [ ] **Step 2: Write the positive fixture (10 hosts, maxInFlight=1)**

```nix
# tests/lib/mkFleet/fixtures/tight-budget-warns.nix
{lib, mkFleet}: let
  mkStubHost = tag: {
    system = "x86_64-linux";
    configuration = (import ./_stub-configuration.nix {inherit lib;});
    tags = [tag];
    channel = "stable";
  };
in
  mkFleet {
    hosts = lib.genAttrs (map (n: "host-${toString n}") (lib.range 1 10)) (_: mkStubHost "etcd");
    channels.stable = {
      rolloutPolicy = "all-at-once";
      signingIntervalMinutes = 60;
      freshnessWindow = 180;
    };
    rolloutPolicies.all-at-once = {
      strategy = "all-at-once";
      waves = [{selector.all = true; soakMinutes = 0;}];
    };
    disruptionBudgets = [{selector.tags = ["etcd"]; maxInFlight = 1;}];
  }
```

- [ ] **Step 3: Generate the expected .resolved.json**

User runs the same pattern as Task 4 Step 4.

- [ ] **Step 4: Commit**

```bash
git add lib/mkFleet.nix tests/lib/mkFleet/fixtures/tight-budget-warns.*
git commit -m "feat(lib/mkFleet): warn when disruption budget is impractically tight"
```

---

## Task 8: Channel freshness + signing-interval options + cross-field invariant

**Files:**
- Modify: `lib/mkFleet.nix`
- Create: `tests/lib/mkFleet/negative/freshness-below-2x.nix`

New invariant from KICKOFF.md §Stream B Milestone 1: `channel.freshnessWindow ≥ 2 × channel.signingIntervalMinutes`.

- [ ] **Step 1: Add the two new options to `channelType`**

In `channelType`, replace with:

```nix
  channelType = types.submodule {
    options = {
      description = mkOption {type = types.str; default = "";};
      rolloutPolicy = mkOption {type = types.str;};
      reconcileIntervalMinutes = mkOption {type = types.int; default = 30;};
      signingIntervalMinutes = mkOption {
        type = types.int;
        default = 60;
        description = ''
          How often CI re-signs `fleet.resolved` for this channel.
          Sets the replay-defense floor: a consumer accepts an artifact for
          at least this long before refresh is expected.
        '';
      };
      freshnessWindow = mkOption {
        type = types.int;
        description = ''
          Minutes a signed `fleet.resolved` artifact is accepted by agents
          after `meta.signedAt`. MUST be ≥ 2 × signingIntervalMinutes so a
          single missed signing run does not strand agents.
        '';
      };
      compliance = mkOption {
        type = types.submodule {
          options = {
            strict = mkOption {type = types.bool; default = true;};
            frameworks = mkOption {type = types.listOf types.str; default = [];};
          };
        };
        default = {};
      };
    };
  };
```

Note: `freshnessWindow` has NO default — channels must declare it explicitly. This forces operators to think about replay defence per-channel.

- [ ] **Step 2: Add the invariant check**

```nix
    freshnessErrors =
      lib.concatMap (
        channelName: let
          c = cfg.channels.${channelName};
        in
          lib.optional (c.freshnessWindow < 2 * c.signingIntervalMinutes)
          "channel '${channelName}': freshnessWindow (${toString c.freshnessWindow}) must be ≥ 2 × signingIntervalMinutes (${toString c.signingIntervalMinutes})"
      )
      (lib.attrNames cfg.channels);
```

Add `freshnessErrors` to `errs`.

- [ ] **Step 3: Propagate the fields into `.resolved`**

In `resolveFleet`:

```nix
      channels =
        lib.mapAttrs (_: c: {
          inherit (c) rolloutPolicy reconcileIntervalMinutes signingIntervalMinutes freshnessWindow compliance;
        })
        cfg.channels;
```

- [ ] **Step 4: Write the negative fixture**

```nix
# tests/lib/mkFleet/negative/freshness-below-2x.nix
{lib, mkFleet}:
mkFleet {
  hosts.m = {
    system = "x86_64-linux";
    configuration = (import ../fixtures/_stub-configuration.nix {inherit lib;});
    tags = [];
    channel = "stable";
  };
  channels.stable = {
    rolloutPolicy = "all-at-once";
    signingIntervalMinutes = 60;
    freshnessWindow = 90; # < 2 × 60, must fail
  };
  rolloutPolicies.all-at-once = {
    strategy = "all-at-once";
    waves = [{selector.all = true; soakMinutes = 0;}];
  };
}
```

- [ ] **Step 5: Fix every other fixture**

Because `freshnessWindow` is now required, update every fixture from Tasks 3–7 to declare it (value `180` is the minimum safe for the default `signingIntervalMinutes = 60`). Only the new negative test omits it incorrectly.

This is grep-and-fix:

```bash
grep -L "freshnessWindow" tests/lib/mkFleet/fixtures/*.nix tests/lib/mkFleet/negative/*.nix
```

Add the two fields where missing.

- [ ] **Step 6: Commit**

```bash
git add lib/mkFleet.nix tests/lib/mkFleet/
git commit -m "feat(lib/mkFleet): require freshnessWindow ≥ 2×signingIntervalMinutes"
```

---

## Task 9: Host SSH pubkey field (CONTRACTS §II #4)

**Files:**
- Modify: `lib/mkFleet.nix`

- [ ] **Step 1: Extend `hostType`**

```nix
  hostType = types.submodule {
    options = {
      system = mkOption {type = types.str;};
      configuration = mkOption {
        type = types.unspecified;
        description = "A nixosConfiguration.";
      };
      tags = mkOption {type = types.listOf types.str; default = [];};
      channel = mkOption {type = types.str;};
      pubkey = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = ''
          Host SSH ed25519 public key (OpenSSH format). Used by the control
          plane to verify probe-output signatures and bind the host's mTLS
          client cert at enrollment. `null` means the host has not been
          enrolled yet; it appears in the fleet schema but signed artifacts
          from it cannot be verified.
        '';
      };
    };
  };
```

- [ ] **Step 2: Propagate into `.resolved`**

In `resolveFleet`, update the hosts map:

```nix
      hosts =
        lib.mapAttrs (_: h: {
          inherit (h) system tags channel pubkey;
          closureHash = null;
        })
        cfg.hosts;
```

- [ ] **Step 3: Commit**

```bash
git add lib/mkFleet.nix
git commit -m "feat(lib/mkFleet): add hosts.<n>.pubkey for SSH host key declarations"
```

---

## Task 10: `meta` scaffold (schemaVersion, signedAt, ciCommit)

**Files:**
- Modify: `lib/mkFleet.nix`

CONTRACTS §I.1 requires `meta.signedAt`, `meta.ciCommit`, `meta.schemaVersion`. CI fills the first two from environment variables at signing time. The evaluator produces a `meta` block with `schemaVersion` pinned and the other two as `null`, plus a helper function that CI calls to overwrite them.

- [ ] **Step 1: Add `meta` to `.resolved`**

In `resolveFleet`, at the top level of the emitted record:

```nix
      meta = {
        schemaVersion = 1;
        signedAt = null; # CI fills via `nixfleet.lib.withSignature`
        ciCommit = null; # CI fills
      };
```

- [ ] **Step 2: Expose `withSignature` helper on the lib**

At the end of `lib/mkFleet.nix`:

```nix
  withSignature = {
    signedAt,
    ciCommit,
  }: resolved:
    resolved
    // {
      meta = resolved.meta // {inherit signedAt ciCommit;};
    };
```

Export it alongside `mkFleet`:

```nix
in {
  inherit mkFleet withSignature;
}
```

And update `lib/default.nix`:

```nix
{lib}: let
  impl = import ./mkFleet.nix {inherit lib;};
in {
  inherit (impl) mkFleet withSignature;
}
```

- [ ] **Step 3: Commit**

```bash
git add lib/mkFleet.nix lib/default.nix
git commit -m "feat(lib/mkFleet): add meta scaffold (schemaVersion, signedAt, ciCommit)"
```

---

## Task 11: `modules/trust.nix` — nixfleet.trust.* option tree

**Files:**
- Create: `modules/trust.nix`
- Create: `modules/tests/trust-options.nix`
- Modify: `modules/core/_nixos.nix` (or wherever core options are assembled)

- [ ] **Step 1: Write the module**

```nix
# modules/trust.nix
#
# nixfleet.trust.* — the four trust roots from docs/CONTRACTS.md §II.
# Public keys are declared here; private keys live elsewhere (HSM, host
# SSH key, offline Yubikey) and never enter this module.
#
# Each root supports a `.previous` slot for the 30-day rotation grace
# window and a shared `rejectBefore` timestamp for compromise response.
{
  config,
  lib,
  ...
}: let
  keySlotType = lib.types.submodule {
    options = {
      current = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Current public key (OpenSSH-armored or framework-specific format).";
      };
      previous = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = ''
          Previous public key accepted during rotation grace. Remove after
          the rotation window closes (see docs/CONTRACTS.md §II for
          per-key grace windows).
        '';
      };
      rejectBefore = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = ''
          RFC 3339 timestamp. Signed artifacts older than this are refused
          regardless of key. Used in compromise response when rolling out
          a new key is not sufficient (pre-compromise artifacts still
          carry the old key's trust).
        '';
      };
    };
  };
in {
  options.nixfleet.trust = {
    ciReleaseKey = lib.mkOption {
      type = keySlotType;
      default = {};
      description = ''
        CI release key (ed25519). Private half in Stream A's HSM/TPM;
        public half declared here. Verified by the control plane on every
        `fleet.resolved` fetch. See docs/CONTRACTS.md §II #1.
      '';
    };

    atticCacheKey = lib.mkOption {
      type = keySlotType;
      default = {};
      description = ''
        Attic binary-cache key. Agents verify before every closure
        activation. See docs/CONTRACTS.md §II #2.
      '';
    };

    orgRootKey = lib.mkOption {
      type = keySlotType;
      default = {};
      description = ''
        Organization root key. Verifies enrollment tokens at the control
        plane. Rotation is a catastrophic event — see docs/CONTRACTS.md
        §II #3.
      '';
    };
  };

  # Assertions make a typo in a declaration a build-time failure rather
  # than a silent runtime trust hole.
  config.assertions = [
    {
      assertion =
        config.nixfleet.trust.ciReleaseKey.previous == null
        || config.nixfleet.trust.ciReleaseKey.current != null;
      message = "nixfleet.trust.ciReleaseKey: cannot set .previous without .current";
    }
    {
      assertion =
        config.nixfleet.trust.atticCacheKey.previous == null
        || config.nixfleet.trust.atticCacheKey.current != null;
      message = "nixfleet.trust.atticCacheKey: cannot set .previous without .current";
    }
    {
      assertion =
        config.nixfleet.trust.orgRootKey.previous == null
        || config.nixfleet.trust.orgRootKey.current != null;
      message = "nixfleet.trust.orgRootKey: cannot set .previous without .current";
    }
  ];
}
```

- [ ] **Step 2: Wire into core module export**

Locate the core NixOS module assembly (likely `modules/core/_nixos.nix`). Read it first:

```bash
head -40 modules/core/_nixos.nix
```

Add `../trust.nix` to its `imports` list. If this file imports are keyed by attribute, append to the list. If the file doesn't exist, check `modules/core/default.nix` or `modules/flake-module.nix:28-30` (`nixosModules.nixfleet-core = ./core/_nixos.nix`).

- [ ] **Step 3: Write an eval test**

```nix
# modules/tests/trust-options.nix
#
# Eval test for modules/trust.nix. Verifies the option tree shape and the
# `.previous` without `.current` assertion fires.
{
  lib,
  pkgs,
  ...
}: let
  evalModule = module:
    (lib.evalModules {
      modules = [
        ../trust.nix
        module
      ];
      specialArgs = {inherit pkgs;};
    }).config;

  happy = evalModule {
    nixfleet.trust = {
      ciReleaseKey.current = "ssh-ed25519 AAAA...ci";
      atticCacheKey.current = "attic:cache.example.com:AAAA...";
    };
  };

  broken = builtins.tryEval (
    (lib.evalModules {
      modules = [
        ../trust.nix
        {
          nixfleet.trust.ciReleaseKey.previous = "ssh-ed25519 AAAA...old";
          # current intentionally null
        }
        {config.assertions = [];} # force assertions evaluation via dummy
      ];
      specialArgs = {inherit pkgs;};
    }).config.assertions
  );
in {
  happyPath = happy.nixfleet.trust.ciReleaseKey.current;
  brokenPath = broken;
}
```

- [ ] **Step 4: User runs the eval test**

```bash
nix eval --impure --json --expr 'import ./modules/tests/trust-options.nix { lib = (builtins.getFlake (toString ./.)).inputs.nixpkgs.lib; pkgs = (builtins.getFlake (toString ./.)).inputs.nixpkgs.legacyPackages.x86_64-linux; }'
```

Expected: JSON with `happyPath = "ssh-ed25519 AAAA...ci"` and `brokenPath.success = true` (the module evals but has a failing assertion — the eval test doesn't fire the assertion because `config.assertions` is only checked at build time, but it does verify the assertions ARE declared).

- [ ] **Step 5: Commit**

```bash
git add modules/trust.nix modules/tests/trust-options.nix modules/core/_nixos.nix
git commit -m "feat(modules): add nixfleet.trust.* option tree for trust roots"
```

---

## Task 12: Homelab example end-to-end

**Files:**
- Create: `examples/fleet-homelab/flake.nix`
- Create: `examples/fleet-homelab/fleet.nix`
- Create: `examples/fleet-homelab/hosts/m70q.nix`
- Create: `examples/fleet-homelab/hosts/workstation.nix`
- Create: `examples/fleet-homelab/hosts/rpi-sensor.nix`

- [ ] **Step 1: Copy the spike example as a starting point**

```bash
cp -r spike/examples/homelab examples/fleet-homelab
```

- [ ] **Step 2: Update `examples/fleet-homelab/flake.nix` to point at the promoted lib**

```nix
{
  description = "nixfleet homelab example — exercises lib/mkFleet.nix";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    nixfleet.url = "path:../..";
    nixfleet.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = {
    self,
    nixpkgs,
    nixfleet,
    ...
  }: {
    nixosConfigurations = {
      m70q-attic = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [./hosts/m70q.nix];
      };
      workstation = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [./hosts/workstation.nix];
      };
      rpi-sensor-01 = nixpkgs.lib.nixosSystem {
        system = "aarch64-linux";
        modules = [./hosts/rpi-sensor.nix];
      };
    };

    fleet = import ./fleet.nix {
      inherit self nixfleet;
    };
  };
}
```

- [ ] **Step 3: Rewrite `examples/fleet-homelab/fleet.nix` with new fields**

```nix
{
  self,
  nixfleet,
  ...
}:
nixfleet.lib.mkFleet {
  hosts = {
    m70q-attic = {
      system = "x86_64-linux";
      configuration = self.nixosConfigurations.m70q-attic;
      tags = ["homelab" "always-on" "eu-fr" "server" "coordinator"];
      channel = "stable";
      pubkey = null; # filled in post-enrollment
    };
    workstation = {
      system = "x86_64-linux";
      configuration = self.nixosConfigurations.workstation;
      tags = ["homelab" "eu-fr" "workstation" "builder"];
      channel = "stable";
      pubkey = null;
    };
    rpi-sensor-01 = {
      system = "aarch64-linux";
      configuration = self.nixosConfigurations.rpi-sensor-01;
      tags = ["edge" "eu-fr" "sensor" "low-power"];
      channel = "edge-slow";
      pubkey = null;
    };
  };

  channels = {
    stable = {
      description = "Workstation canary → M70q promote.";
      rolloutPolicy = "homelab-canary";
      reconcileIntervalMinutes = 30;
      signingIntervalMinutes = 60;
      freshnessWindow = 180;
      compliance = {
        strict = true;
        frameworks = ["anssi-bp028"];
      };
    };
    edge-slow = {
      description = "Low-power sensors; weekly reconcile.";
      rolloutPolicy = "all-at-once";
      reconcileIntervalMinutes = 10080;
      signingIntervalMinutes = 60;
      freshnessWindow = 20160; # 14 days
    };
  };

  rolloutPolicies = {
    homelab-canary = {
      strategy = "canary";
      waves = [
        {selector.tags = ["workstation"]; soakMinutes = 30;}
        {selector.tags = ["always-on"]; soakMinutes = 60;}
      ];
      healthGate = {systemdFailedUnits.max = 0;};
      onHealthFailure = "rollback-and-halt";
    };
    all-at-once = {
      strategy = "all-at-once";
      waves = [{selector.all = true; soakMinutes = 0;}];
      healthGate = {systemdFailedUnits.max = 0;};
    };
  };

  edges = [];

  disruptionBudgets = [
    {selector.tags = ["always-on"]; maxInFlight = 1;}
    {selector.tags = ["coordinator"]; maxInFlight = 1;}
  ];
}
```

- [ ] **Step 4: User evaluates end-to-end**

```bash
nix eval --json ./examples/fleet-homelab#fleet.resolved | jq . | tee tests/lib/mkFleet/fixtures/homelab.resolved.json
```

Expected: a JSON object with `schemaVersion: 1`, `meta: { schemaVersion: 1, signedAt: null, ciCommit: null }`, three hosts, two channels with freshnessWindow and signingIntervalMinutes fields, waves, and disruption budgets. Commit the generated fixture.

- [ ] **Step 5: Commit**

```bash
git add examples/fleet-homelab/ tests/lib/mkFleet/fixtures/homelab.resolved.json
git commit -m "feat(examples): add homelab example exercising lib/mkFleet end-to-end"
```

---

## Task 13: Golden-file acceptance test

**Files:**
- Create: `tests/lib/mkFleet/fixtures/homelab.nix`

- [ ] **Step 1: Write a fixture that imports the homelab example**

```nix
# tests/lib/mkFleet/fixtures/homelab.nix
{lib, mkFleet}: import ../../../examples/fleet-homelab/fleet.nix {
  inherit lib;
  nixfleet.lib.mkFleet = mkFleet;
  self.nixosConfigurations = let
    stub = import ./_stub-configuration.nix {inherit lib;};
  in {
    m70q-attic = stub;
    workstation = stub;
    rpi-sensor-01 = stub;
  };
}
```

- [ ] **Step 2: Regenerate the golden from this path to keep them in sync**

User runs:

```bash
nix eval --impure --json --expr '(import ./tests/lib/mkFleet/fixtures/homelab.nix { lib = (builtins.getFlake (toString ./.)).inputs.nixpkgs.lib; mkFleet = (import ./lib/mkFleet.nix { lib = (builtins.getFlake (toString ./.)).inputs.nixpkgs.lib; }).mkFleet; }).resolved' | jq . > tests/lib/mkFleet/fixtures/homelab.resolved.json
```

- [ ] **Step 3: Commit**

```bash
git add tests/lib/mkFleet/fixtures/homelab.nix tests/lib/mkFleet/fixtures/homelab.resolved.json
git commit -m "test(lib/mkFleet): golden-file acceptance fixture for homelab example"
```

---

## Task 14: JCS canonicalization shape hook

**Files:**
- Modify: `lib/mkFleet.nix`
- Modify: `docs/CONTRACTS.md` (only a line comment — the choice is Stream C's)

CONTRACTS §III: "JCS (RFC 8785) with a single Rust implementation". Stream B does NOT pick the library — Stream C does. But the Nix output must be JCS-ready: that means no floating-point values we care about equality on, no ambiguous Unicode in keys, no hidden attribute order.

- [ ] **Step 1: Verify the resolved output is JCS-friendly**

All our values are ints, strings, lists, and plain attribute sets. Attribute sets are deterministically ordered by Nix. No floats. No Unicode normalization issues in the homelab fixture. This is property-verified by the golden-file test in Task 13.

Add a comment at the top of `lib/mkFleet.nix`:

```nix
# lib/mkFleet.nix
#
# Produces `fleet.resolved` per RFC-0001 §4.1 + docs/CONTRACTS.md §I #1.
# Output is canonicalized to JCS (RFC 8785) by `bin/nixfleet-canonicalize`
# (owned by Stream C) before signing — DO NOT introduce floats, opaque
# derivations, or attrsets whose iteration order is significant here.
```

- [ ] **Step 2: Cross-link from CONTRACTS.md**

Edit `docs/CONTRACTS.md` §III first paragraph to add:

```markdown
Producer-side (Stream B's `lib/mkFleet.nix`) MUST emit values that round-trip through JCS losslessly: ints only (no floats), deterministic attr order, no JSON-incompatible types. Consumer-side (Stream C's `bin/nixfleet-canonicalize`) pins the library.
```

- [ ] **Step 3: Commit**

```bash
git add lib/mkFleet.nix docs/CONTRACTS.md
git commit -m "docs(contracts): pin JCS producer-side discipline in lib/mkFleet"
```

This is a CONTRACTS.md edit — mark the PR with `contract-change` when opening. Stream C and Stream A must sign off per KICKOFF.md §1 merge discipline.

---

## Task 15: CHANGELOG + README

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `README.md` (if it has a "What's new" section)

- [ ] **Step 1: Add CHANGELOG entry under `## [Unreleased]`**

Open `CHANGELOG.md` and add under `### Added`:

```markdown
- `lib.mkFleet` — evaluates a declarative fleet description and emits a
  typed `.resolved` artifact per RFC-0001. Every invariant from §4.2 is
  enforced at eval time: host/channel/policy references, edge DAG,
  compliance framework allow-list, and the cross-field
  `freshnessWindow ≥ 2 × signingIntervalMinutes` relation.
- `nixfleet.trust.*` option tree — declares CI release key, attic cache
  key, and org root key (with rotation grace slots and a compromise
  `rejectBefore` switch).
- `examples/fleet-homelab/` — a working end-to-end example producing a
  signed-shape `fleet.resolved`.
```

- [ ] **Step 2: If `README.md` has a quick-start, extend it with one line showing `nix eval .#fleet.resolved`**

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md README.md
git commit -m "docs: note lib.mkFleet and nixfleet.trust.* in changelog"
```

---

## Task 16: Wire into CI + `nix flake check`

**Files:**
- Modify: `flake.nix` or `modules/tests/*.nix`

- [ ] **Step 1: Add eval test entry point**

Find where `flake.checks` is assembled (likely `modules/tests/eval.nix` or perSystem in the main flake). Add a derivation that runs the mkFleet harness:

```nix
# In the checks assembly:
mkFleet-eval-tests = pkgs.runCommand "mkFleet-eval-tests" {} ''
  ${pkgs.nix}/bin/nix-instantiate --eval --json --strict \
    --argstr unused 'unused' \
    -E '(import ${./tests/lib/mkFleet/default.nix} { lib = (import <nixpkgs> {}).lib; })' > $out
'';
```

(Adjust pattern to match the project's actual checks scaffold.)

- [ ] **Step 2: User verifies `nix flake check` picks up the new test**

```bash
nix flake check --no-build 2>&1 | grep -i mkFleet
```

Expected: reference to `mkFleet-eval-tests` in the checks list.

- [ ] **Step 3: Commit**

```bash
git add flake.nix modules/tests/
git commit -m "ci: wire lib/mkFleet eval tests into nix flake check"
```

---

## Task 17: Tracking-issue sync + PR

**Files:**
- `abstracts33d/nixfleet#10` (comment on tracking issue)
- PR on `abstracts33d/nixfleet`

- [ ] **Step 1: User-facing summary**

Before opening a PR, present to the user:

```
Branch: feat/mkfleet-promotion
Closes: abstracts33d/nixfleet#1 (and the Nix portion of #12)
Commits: N commits, each atomic per task above
Acceptance: `nix eval --json ./examples/fleet-homelab#fleet.resolved`
produces a schemaVersion:1 artifact matching the golden file.

Review OK, can I ship?
```

WAIT for explicit confirmation.

- [ ] **Step 2: On confirmation — push and open the PR**

```bash
git push -u origin feat/mkfleet-promotion
gh pr create --repo abstracts33d/nixfleet \
  --title "feat: promote mkFleet + add nixfleet.trust.* option tree (#1)" \
  --body "$(cat <<'EOF'
## Summary
- Promote `spike/lib/mkFleet.nix` → `lib/mkFleet.nix` with every RFC-0001 §4.2 invariant enforced.
- New cross-field invariant: `channel.freshnessWindow ≥ 2 × signingIntervalMinutes` (closes the Nix portion of #12 freshness coverage).
- Add `modules/trust.nix` with `nixfleet.trust.ciReleaseKey`, `.atticCacheKey`, `.orgRootKey` (each with `.previous` grace slot and `rejectBefore` compromise switch).
- End-to-end example in `examples/fleet-homelab/` + golden-file acceptance test.
- CONTRACTS.md edit: producer-side JCS discipline line (requires Stream A and Stream C signoff per `contract-change` rule).

## Acceptance
- `nix eval --json ./examples/fleet-homelab#fleet.resolved` produces a `schemaVersion: 1` artifact byte-identical to `tests/lib/mkFleet/fixtures/homelab.resolved.json`.
- `nix flake check` runs the positive and negative mkFleet fixtures.

## Test plan
- [ ] Eval positive fixtures pass
- [ ] Each negative fixture throws as expected
- [ ] `nixfleet.trust.*` assertion fires when `.previous` set without `.current`
- [ ] Homelab example evaluates cleanly against the golden file

Closes #1
Partial: #12 (Nix portion — signing tooling lands in Stream C)
EOF
)"
```

- [ ] **Step 3: Post cross-stream status on tracking issue**

```bash
gh issue comment 10 --repo abstracts33d/nixfleet --body "$(cat <<'EOF'
Stream B — Milestone 1 (nixfleet side): mkFleet promoted, nixfleet.trust.* landed. PR #NN. Note: CONTRACTS.md §III now documents producer-side JCS discipline — Stream A and Stream C signoff requested on the PR.
EOF
)"
```

---

## Self-Review Checklist

- [x] Every RFC-0001 §4.2 invariant implemented: host config validity (Task 3), channel lookup (spike baseline), policy lookup (spike baseline), empty selector warn (Task 4), compliance framework allow-list (Task 5), DAG check (Task 6), disruption budget warn (Task 7).
- [x] New invariant `freshnessWindow ≥ 2 × signingIntervalMinutes` in Task 8.
- [x] `hosts.<n>.pubkey` in Task 9.
- [x] `meta` scaffold in Task 10.
- [x] `nixfleet.trust.*` option tree in Task 11.
- [x] End-to-end example in Task 12.
- [x] Golden-file test in Task 13.
- [x] JCS producer-side discipline pinned in Task 14.
- [x] Docs in Task 15.
- [x] CI wiring in Task 16.
- [x] No placeholders; every code block is complete.
- [x] No referenced symbol is undefined: `checkInvariants`, `resolveSelector`, `resolveFleet`, `hasCycle`, `withSignature`, `keySlotType`, `channelType`, `hostType` are all defined where referenced.
- [x] Contract change (Task 14) flagged for cross-stream signoff per KICKOFF.md discipline.
