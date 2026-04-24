# nixfleet spike — `mkFleet` + reconciler prototype

A minimal, runnable implementation of RFC-0001 (declarative fleet schema) and
RFC-0002 §4 (decision procedure), wired to a homelab topology: M70q attic
server, Ryzen workstation, RPi sensor.

## Layout

```
spike/
├── flake.nix                        flake-parts entry point
├── lib/
│   └── mkFleet.nix                  RFC-0001 schema as NixOS module types
├── examples/
│   └── homelab/
│       ├── fleet.nix                topology, declared
│       └── hosts/                   stub nixosConfigurations
├── fixtures/
│   ├── fleet.resolved.json          sample projection output
│   └── observed.json                simulated control-plane state
└── reconciler/
    ├── Cargo.toml
    └── src/main.rs                  RFC-0002 §4 decision procedure
```

## Running

```
# Project the fleet to a resolved JSON artifact:
nix eval --json .#fleet.resolved > fixtures/fleet.resolved.json

# Run one reconcile tick against the fixtures:
cd reconciler && cargo run -- ../fixtures/fleet.resolved.json ../fixtures/observed.json
```

## Exercising state transitions

The reconciler is a pure function `(Fleet, Observed) -> Vec<Action>` with no
I/O beyond reading the two JSON files. Every transition in RFC-0002 §4 can be
exercised by editing `fixtures/observed.json` and re-running.

Examples:

- `workstation` state → `"Activating"`, `m70q-attic` offline: no actions
  (wave in progress, offline host stays Queued).
- `workstation` → `"Soaked"`: wave 0 promotes; wave 1 (`m70q-attic`) hits the
  offline skip branch.
- Bring `m70q-attic` online and the disruption-budget branch dispatches.
- Set `workstation` to `"Failed"`: halt transition fires.

## Next extensions

1. **`nix flake check` integration** — wrap `resolveFleet` in a check
   derivation that also parses through the Rust binary to verify schema
   compatibility.
2. **Compliance static gate stub** — walk each host's `configuration.config`
   and run typed controls.
3. **`--trace` output mode** — annotate each action with the RFC-0002 §4
   sub-step that produced it.
4. **Scenario runner** — iterate `(observed, expected-plan)` pairs and assert
   equivalence. Regression protection before committing to state-machine
   changes.

Everything else (HTTP server, mTLS, SQLite persistence, actual agent) is
scaffolding around this core. Get the pure decision procedure right in
fixture-land first.
