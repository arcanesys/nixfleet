# Technical Architecture

Deep-dive into NixFleet's design decisions, framework internals, and Nix gotchas.

For a high-level overview, see [ARCHITECTURE.md](ARCHITECTURE.md). For getting started, see [QUICKSTART.md](QUICKSTART.md). For full docs, see [docs/mdbook/](docs/mdbook/).

## NixFleet Framework

### mkHost Internals

`mkHost` is a closure that binds the framework's flake inputs, then returns a standard `nixosSystem` or `darwinSystem`:

```
mkHost { hostName, platform, hostSpecValues, hardwareModules, extraModules }
  |
  +-- platform detection (x86_64-linux, aarch64-linux -> nixosSystem; aarch64-darwin -> darwinSystem)
  |
  +-- module injection:
  |     - hostSpec module (binds hostSpecValues as options)
  |     - core/_nixos.nix or core/_darwin.nix
  |     - scopes/_base.nix, scopes/_impermanence.nix
  |     - services: _agent.nix, _control-plane.nix
  |     - disko + impermanence modules (NixOS only)
  |     - home-manager module
  |     - hardwareModules (user-provided)
  |     - extraModules (user-provided)
  |
  +-- returns nixpkgs.lib.nixosSystem { ... } or darwin.lib.darwinSystem { ... }
```

### hostSpec Options

The framework defines base hostSpec options in `host-spec-module.nix`:

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `userName` | str | -- | Primary username |
| `hostName` | str | -- | Machine hostname |
| `timeZone` | str | `"UTC"` | IANA timezone |
| `locale` | str | `"en_US.UTF-8"` | System locale |
| `keyboardLayout` | str | `"us"` | XKB layout |
| `sshAuthorizedKeys` | list of str | `[]` | SSH public keys |
| `secretsPath` | str or null | null | Secrets repo path hint |
| `isMinimal` | bool | false | Suppress base packages |
| `isDarwin` | bool | false | macOS host |
| `isImpermanent` | bool | false | Enable impermanence |
| `isServer` | bool | false | Headless server |
| `hashedPasswordFile` | str or null | null | Primary user password file |
| `rootHashedPasswordFile` | str or null | null | Root password file |

Fleet repos extend hostSpec with their own options (isDev, isGraphical, useNiri, etc.) by adding option declarations in their modules.

### Scope Pattern

Scopes are plain NixOS/Darwin/HM modules that self-activate via `lib.mkIf`:

```nix
# modules/scopes/_example.nix
{ config, lib, ... }: let
  hS = config.hostSpec;
in {
  config = lib.mkIf hS.someFlag {
    # configuration here
  };
}
```

Key properties:
- No deferred module registration -- scopes are imported directly by `mkHost`
- Self-activation via `lib.mkIf hS.<flag>` -- no external wiring
- Persist paths co-located with their scope, not centralized
- The `_` prefix excludes scope files from import-tree auto-import; `mkHost` imports them explicitly

### Home-Manager Integration

`mkHost` injects a home-manager module that:
1. Creates a user configuration for `hostSpec.userName`
2. Passes `hostSpec` to HM via `extraSpecialArgs`
3. Includes framework HM modules and user-provided HM modules
4. Fleet repos add their own HM programs (starship, nvim, tmux, etc.)

### Input Follows Strategy

Fleet repos follow the framework's pinned versions to avoid version conflicts:

```nix
inputs = {
  nixfleet.url = "github:your-org/nixfleet";
  nixpkgs.follows = "nixfleet/nixpkgs";
  home-manager.follows = "nixfleet/home-manager";
};
```

Framework inputs are passed via `specialArgs = { inherit inputs; }`. Fleet repos access these as the `inputs` argument. Fleet-specific customization uses hostSpec extensions and plain NixOS modules.

### Org Defaults

Instead of `mkOrg`, org-wide defaults are plain `let` bindings in the fleet's `flake.nix`:

```nix
let
  orgDefaults = {
    userName = "admin";
    timeZone = "Europe/Paris";
    sshAuthorizedKeys = [ "ssh-ed25519 AAAA..." ];
  };
in {
  nixosConfigurations.host-a = mkHost {
    hostSpecValues = orgDefaults // { hostName = "host-a"; isDev = true; };
    ...
  };
}
```

Host values override org defaults via `//` (attrset merge). No priority layers needed.

## Rust Workspace

Four crates in a Cargo workspace at the repo root:

| Crate | Binary | Purpose |
|-------|--------|---------|
| `agent/` | `nixfleet-agent` | Runs on each managed host. State machine: Idle -> Checking -> Fetching -> Applying -> Verifying -> Reporting |
| `control-plane/` | `nixfleet-control-plane` | Axum HTTP server. Machine registry, deployment orchestration |
| `cli/` | `nixfleet` | Operator CLI. Deploy, status, rollback commands |
| `shared/` | (library) | `nixfleet-types`: shared data types, API contracts |

### Agent <-> Control Plane Communication

```
Agent (on host)                    Control Plane (central)
    |                                      |
    +-- POST /api/v1/machines/{id}/register ---->|  Register with hostname + platform
    |                                            |
    +-- GET  /api/v1/machines/{id}/desired-generation -->|  Poll for desired state
    |                                            |
    +-- POST /api/v1/machines/{id}/report ------>|  Report deploy result
    |                                      |
    +-- heartbeat loop ------------------>|  Liveness signal
```

### Machine Lifecycle States

Two state models operate at different levels:

**Fleet lifecycle** (`shared/src/lib.rs` -- `MachineLifecycle`): Tracks machine status from the control plane's perspective.

```
Pending -> Provisioning -> Active -> Maintenance -> Decommissioned
                              |           |
                              +-----------+  (bidirectional)
```

**Agent state machine** (`agent/src/state.rs` -- `AgentState`): Tracks what the agent is doing on each host.

```
Idle -> Checking -> Fetching -> Applying -> Verifying -> Reporting -> Idle
                       |            |            |
                       +-- Idle     +-- RollingBack --+
                     (fetch error)    (switch/health failed)
```

## Nix Gotchas

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

Opinionated inputs (catppuccin, nix-index-database, wrapper-modules, nix-homebrew) are added by fleets that need them.
