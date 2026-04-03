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

Don't use `mkDefault` for `networking.useDHCP = false` in core — it conflicts with `hardware-configuration.nix`'s `mkDefault true` (same priority). Use plain value.

## Infrastructure Scopes

Four backend-agnostic scopes added as plain NixOS modules:

- **Firewall** (`_firewall.nix`): enables nftables, SSH rate limiting (5/min), drop logging. Auto-activates on non-minimal hosts via `!isMinimal`.
- **Secrets** (`_secrets.nix`): computes `resolvedIdentityPaths` from hostSpec (host key primary, user key fallback on workstations). Handles impermanence persistence and boot ordering. Fleet repos bring their own backend (agenix, sops-nix).
- **Backup** (`_backup.nix`): provides a systemd timer, pre/post hooks, health check pings, and status reporting. Fleet repos wire the actual backup command (restic, borgbackup, etc.).
- **Monitoring** (`_monitoring.nix`): wraps Prometheus node exporter with fleet-tuned collector defaults.

Pattern: framework owns wiring, fleet repos pick tools.

### Prometheus Metrics

Both the agent and the control plane expose Prometheus metrics. The control plane serves `GET /metrics` on its listen address (always available). The agent exposes metrics on a configurable `metricsPort` with an optional firewall opening via `metricsOpenFirewall`.

### Auth Route Split

The control plane splits routes by caller:

- **Agent-facing routes**: authenticated via mTLS (client certificate). No API key required.
- **Admin routes**: authenticated via API key. Used by the CLI and operators.

This separation ensures agents cannot be accidentally blocked by API key rotation, and admin endpoints cannot be reached by machine credentials alone.

## Rust Workspace

Four crates in a Cargo workspace at the repo root:

| Crate | Binary | Purpose |
|-------|--------|---------|
| `agent/` | `nixfleet-agent` | State machine on each managed host: poll → fetch → apply → verify → report |
| `control-plane/` | `nixfleet-control-plane` | Axum HTTP server. Machine registry, rollout orchestration, audit log |
| `cli/` | `nixfleet` | Operator CLI. Deploy, status, rollback, rollout, machines |
| `shared/` | (library) | `nixfleet-types`: shared data types, API contracts |

### Agent ↔ Control Plane Communication

```
Agent (on host)                    Control Plane (central)
    |                                      |
    +-- POST /api/v1/machines/{id}/register ---->|  Register with hostname + platform
    |                                            |
    +-- GET  /api/v1/machines/{id}/desired-generation -->|  Poll for desired state
    |                                            |
    +-- POST /api/v1/machines/{id}/report ------>|  Report deploy result + health
    |                                      |
    +-- health reports (continuous) ------->|  Health status between deploys
```

### Machine Lifecycle States

**Fleet lifecycle** (`MachineLifecycle`): Tracks machine status from the control plane's perspective.

```
Pending → Provisioning → Active ↔ Maintenance → Decommissioned
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
| `agenix` | Age-encrypted secrets |
| `nixos-anywhere` | Remote NixOS installation via SSH |
| `nixos-hardware` | Hardware-specific optimizations |
| `lanzaboote` | Secure Boot |
| `treefmt-nix` | Multi-language formatting |

Fleet repos add their own inputs as needed.
