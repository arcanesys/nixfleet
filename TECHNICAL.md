# Technical Notes

Implementation details, Nix gotchas, and internal architecture for contributors.

For the user-facing documentation, see [docs/mdbook/](docs/mdbook/). For a high-level overview, see [ARCHITECTURE.md](ARCHITECTURE.md).

## Nix Gotchas

Lessons learned from building with Nix and flake-parts. These are traps that are easy to fall into and hard to debug.

### perSystem and unfree

`perSystem` pkgs don't inherit `nixpkgs.config.allowUnfree` from NixOS. Unfree packages must go in NixOS/HM modules, not perSystem apps.

### Backup file collisions

`backupFileExtension` creates fixed-name backups that block future activations. Use `backupCommand` with timestamped names and pruning:

```nix
backupCommand = ''mv {} {}.nbkp.$(date +%Y%m%d%H%M%S) && ls -t {}.nbkp.* 2>/dev/null | tail -n +6 | xargs -r rm -f'';
```

### home.file force

SSH public keys and other managed files that should always be overwritten: use `force = true`.

### networking.interfaces guard

`networking.interfaces."${name}".useDHCP` crashes if name is empty. Guard with `lib.mkIf (hS.networking ? interface)`.

### networking.useDHCP priority

Don't use `mkDefault` for `networking.useDHCP = false` in core -- it conflicts with `hardware-configuration.nix`'s `mkDefault true` (same priority). Use plain value.

## Rust Workspace

Four crates in a Cargo workspace at the repo root:

| Crate | Binary | Purpose |
|-------|--------|---------|
| `agent/` | `nixfleet-agent` | State machine on each managed host: poll → fetch → apply → verify → report |
| `control-plane/` | `nixfleet-control-plane` | Axum HTTP server. Machine registry, rollout orchestration, audit log |
| `cli/` | `nixfleet` | Operator CLI. Deploy, status, rollback, rollout, release, machines, bootstrap, init |
| `shared/` | (library) | `nixfleet-types`: shared data types, API contracts |

### Agent ↔ Control Plane Communication

```
Agent (on host)                    Control Plane (central)
    |                                      |
    +-- GET  /api/v1/machines/{id}/desired-generation -->|  Poll for desired state
    |                                            |
    +-- POST /api/v1/machines/{id}/report ------>|  Report deploy result + health + tags
    |                                      |
    +-- health reports (periodic) -------->|  Health status between deploys

CLI (operator)                     Control Plane (central)
    |                                      |
    +-- POST /api/v1/machines/{id}/register --->|  Pre-register a machine (admin)
    +-- PATCH /api/v1/machines/{id}/lifecycle ->|  Change lifecycle state (admin)
```

### Machine Lifecycle States

**Fleet lifecycle** (`MachineLifecycle`): Tracks machine status from the control plane's perspective.

```
Pending → Provisioning → Active ↔ Maintenance
  |            |            |           |
  +→ Active    +→ Pending   +→ Decom.   +→ Decom.
  +→ Decom.
```

**Agent state machine** (`AgentState`): Tracks what the agent is doing on each host.

```
Idle → Checking → Fetching → Applying → Verifying → Reporting → Idle
                                            ↓             ↓
                                        RollingBack ← (health failure)
```

For detailed documentation of agent states, health checks, and rollout strategies, see the [Deploying guide](docs/mdbook/guide/deploying/agent.md).

## Flake Inputs

| Input | Purpose |
|-------|---------|
| `nixpkgs` | Package repository (nixos-unstable) |
| `darwin` | nix-darwin macOS system config |
| `home-manager` | User environment management |
| `flake-parts` | Module system for flake outputs |
| `import-tree` | Auto-import directory tree as modules |
| `disko` | Declarative disk partitioning |
| `impermanence` | Ephemeral root filesystem |
| `nixos-anywhere` | Remote NixOS installation via SSH |
| `nixos-hardware` | Hardware-specific optimizations |
| `lanzaboote` | Secure Boot |
| `treefmt-nix` | Multi-language formatting |
| `microvm` | MicroVM support (microvm.nix) |

Fleet repos add their own inputs as needed (e.g. `agenix` or `sops-nix` for secrets).
