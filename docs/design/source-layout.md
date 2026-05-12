# Nix source layout

The Nix half of the codebase is split into four layers. The split is structural, not stylistic - each directory plays a distinct role in the API surface and import graph. When adding code, this doc tells you which layer it belongs in.

If you're looking for the *runtime* architecture (components, trust flow, build order), read [`./architecture.md`](./architecture.md) instead. This doc is about source organization for contributors.

## The four layers

| Directory | Role | Imported by |
|---|---|---|
| `lib/` | **Public flake API.** Function-style helpers consumers call from their own `flake.nix`. | Consumer fleets via `nixfleet.lib.*` |
| `modules/scopes/` | **Auto-included service modules.** Contributed to every host `mkHost` builds; gated by `enable` flags. | Implicitly, through `mkHost` |
| `contracts/` | **Typed schemas with no implementation.** Declare options that other code reads and writes; carry no runtime behavior. | `mkHost` (auto-imports all) |
| `impls/` | **Opt-in implementations of contract schemas.** A fleet picks at most one impl per family. | Consumer fleet, explicitly |

The split is visible in `modules/flake-module.nix`: `flake.lib` exposes the `lib/` layer; `flake.scopes.*` exposes the `impls/` layer as named alternatives; `modules/scopes/*` are wired into `mkHost`'s default module list; `contracts/*` are auto-imported via `mkHost`'s prelude.

## When does code go where?

### `lib/` - public API

Goes here if it's a **function** consumers call: `mkHost`, `mkFleet`, `mkVmApps`, `mergeFleets`, `withSignature`. Pure Nix functions, no NixOS module evaluation. Exposed at `nixfleet.lib.<name>` via `lib/default.nix`.

Rule of thumb: if the body is `fleetConfig: { ... }` or `args: { ... }` and never declares `options.*` or `config.*`, it belongs here.

Example - the recently split `mkVmApps` is a function returning a flake-apps attrset; its implementation lives in `lib/mk-vm-apps.nix` plus the `lib/vm-platform.nix` / `lib/vm-helpers.sh` / `lib/vm-scripts/` siblings.

### `modules/scopes/<scope>/` - auto-included service modules

Goes here if it's a **NixOS module the framework wants every relevant host to evaluate**, gated by an `enable` flag. The agent, the control plane, the operator user, the cache pinning, the microvm host - all auto-included by `mkHost` so consumers don't have to remember to `imports = [ ... ]` every relevant module on every host.

Each file is a complete NixOS module: declares its `services.<name>.*` options, plus the `config = lib.mkIf cfg.enable { ... }` block that lights up when the consumer flips the flag.

Naming convention: file starts with `_` (e.g. `_agent.nix`, `_control-plane.nix`) to keep the import-tree merge predictable.

### `contracts/` - typed schemas, no implementation

Goes here if it's an **option schema other code depends on, but the schema itself has no behavior**. The schema declares what fields exist and what types they take; downstream impls or service modules read those fields and do the actual work.

Today: `hostSpec` (host identity), `nixfleet.persistence.*` (persisted-paths schema), `nixfleet.trust.*` (trust-root keys + algorithms).

Rule of thumb: if removing this file's `config = ...` block would change no observed runtime behavior, it's a contract.

Putting a contract here (rather than inline in a service module) decouples readers from writers. Multiple service modules can contribute `nixfleet.persistence.directories` without knowing whether the consumer fleet uses impermanence, ZFS rollback, or no impermanence at all. The impl reads those contributions and translates.

### `impls/<family>/<impl>.nix` - opt-in implementations

Goes here if it's a **concrete implementation of a contract schema, where multiple alternatives could exist** and a consumer fleet picks one explicitly.

Today's families:

| Family | Contract | Impls |
|---|---|---|
| `persistence` | `contracts/persistence.nix` | `impermanence` |
| `keyslots` | (none yet - contract is implicit in TPM module) | `tpm` |
| `gitops` | source-URL builders for `services.nixfleet-control-plane.channelRefsSource` | `forgejo` (also aliased as `gitea`) |
| `secrets` | identity-path resolution for agenix/sops backends | `secrets` (single canonical impl) |

Each impl is exposed as `flake.scopes.<family>.<impl>` (see `modules/flake-module.nix`). Consumer fleets opt in by importing exactly one per family:

```nix
imports = [ inputs.nixfleet.scopes.persistence.impermanence ];
```

Sibling entries are mutually exclusive. Adding a third impl to an existing family is when the family-vs-impl boundary earns its keep - write a new file alongside the existing one, no schema change required.

## What does **not** go in any of these

- **Rust code.** Lives in `crates/`, independent build graph.
- **Per-host NixOS configuration.** Lives in the *consumer* fleet's flake (e.g. `cache.nix`, `workstation.nix`). The framework doesn't ship host configs; it ships the machinery to build them.
- **Test fixtures and scenarios.** Live in `tests/harness/`, not in any framework layer.
- **Internal flake plumbing** that isn't part of the public API or a NixOS module: `modules/apps.nix` (the `validate` flake-app), `modules/formatter.nix` (treefmt config), `modules/rust-packages.nix` (crane wiring). These live at `modules/` root, not in a layer subdirectory.

## Cross-references

- `modules/flake-module.nix` - the wire-up that turns these directories into flake outputs.
- `lib/mk-host.nix` - the function that auto-includes `modules/scopes/*` and `contracts/*` for each host.
- [`./contracts.md`](./contracts.md) - the cross-stream artifact contracts (different sense of "contract" - wire formats and signed artifacts, not Nix option schemas).

## Dependency pinning policy

Fleet repos that consume `nixfleet` inherit `nixpkgs`, `home-manager`, `disko`, and the other shared inputs via `inputs.<name>.follows = "nixfleet/<name>"` rather than pinning their own copies. The framework modules are evaluated against the exact revisions declared in this repo's `flake.lock`; an independent consumer pin can drift on option renames, type changes, or removed modules between the revisions the framework was tested against and the ones the consumer evaluates with.

The practical contract: `nix flake update nixfleet` in a fleet repo updates every shared dependency in one step. The framework commits to staying current against `nixos-unstable` so consumers are not pinned to a stale tree. Fleet-specific inputs (themes, editor plugins, things the framework does not know about) are pinned independently by the consumer - the `follows` chain only covers what the framework guarantees to test against.
