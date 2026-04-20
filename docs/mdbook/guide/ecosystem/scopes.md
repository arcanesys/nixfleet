# Scopes & Roles

NixFleet uses a scope system to compose host configurations. Scopes ship in the [nixfleet-scopes](https://github.com/arcanesys/nixfleet-scopes) companion repository - a standalone collection of infrastructure modules, roles, and disk templates that work with any NixFleet-managed host.

Scopes are NixOS modules that self-activate based on configuration flags. Each scope wraps its `config` block in `lib.mkIf` so it produces no configuration when its condition is false. Options live under `nixfleet.*`. Roles compose scopes and set defaults with `lib.mkDefault` - consumers override with `lib.mkForce` when needed.

> **Repository:** [github.com/arcanesys/nixfleet-scopes](https://github.com/arcanesys/nixfleet-scopes) - MIT licensed, works standalone or via `inputs.nixfleet.scopes` re-export.

## Framework Service Scopes

These ship with NixFleet and are auto-included by `mkHost` (disabled by default).

| Scope | Options | Description |
|-------|---------|-------------|
| Agent | `services.nixfleet-agent.*` | Deploy cycle daemon - polls CP, applies generations, reports health |
| Agent (Darwin) | `services.nixfleet-agent.*` | macOS variant using launchd |
| Control Plane | `services.nixfleet-control-plane.*` | Axum HTTP with mTLS, SQLite, RBAC for fleet orchestration |
| Cache Server | `services.nixfleet-cache-server.*` | Harmonia-based Nix binary cache serving from local store |
| Cache | `services.nixfleet-cache.*` | Nix substituter pointing to fleet cache |
| MicroVM Host | `services.nixfleet-microvm-host.*` | MicroVM hypervisor with bridge networking, DHCP, and NAT |

The impermanence scope from nixfleet-scopes is also auto-imported by `mkHost`. It is inert unless `nixfleet.impermanence.enable` is set.

## Infrastructure Scopes

From [nixfleet-scopes](https://github.com/arcanesys/nixfleet-scopes). Import via roles or individually.

| Scope | Namespace | Description |
|-------|-----------|-------------|
| base | `nixfleet.base` | Universal CLI tools (ifconfig, netstat, xdg-utils). Darwin and HM variants available. |
| operators | `nixfleet.operators` | Multi-user management - primary user, SSH keys, sudo, shell, HM routing, role groups |
| firewall | `nixfleet.firewall` | nftables backend, SSH rate limiting (5/min), drop logging, microVM bridge forwarding |
| secrets | `nixfleet.secrets` | Backend-agnostic identity paths for agenix/sops-nix, boot ordering, key validation |
| backup | `nixfleet.backup` | Timer scaffolding with restic and borgbackup backends, pre/post hooks, health pings |
| monitoring | `nixfleet.monitoring` | Prometheus node exporter with fleet-tuned collector defaults |
| monitoring-server | `nixfleet.monitoring.server` | Prometheus server with scrape configs, retention, and built-in alert rules |
| impermanence | `nixfleet.impermanence` | Btrfs root wipe + system persist paths (/etc/nixos, /var/lib/systemd, /var/log, etc.) |
| home-manager | `nixfleet.home-manager` | HM integration - useGlobalPkgs/useUserPackages defaults, fans out profileImports to HM-enabled operators |
| disko | `nixfleet.disko` | Disko NixOS module injection (inert without `disko.devices`) |
| o11y | `nixfleet.o11y` | Metrics remote-write (vmagent to VictoriaMetrics/Mimir) + journal log shipping |
| vpn | `nixfleet.vpn` | Profile-driven VPN framework with wireguard driver |
| compliance | `nixfleet.compliance` | Filesystem integration for compliance evidence - persists evidence dir, sets configurationRevision |
| generation-label | `nixfleet.generationLabel` | Rich boot entry labels from flake metadata (date, rev, deterministic codename) |
| remote-builders | `nixfleet.distributedBuilds` | Cross-platform distributed build delegation (handles Determinate Nix on Darwin) |
| hardware | `nixfleet.hardware` | Auto-imports hardware sub-modules: microcode, bluetooth, nvidia, wake-on-LAN, memory/zram, legacy boot |
| terminal-compat | `nixfleet.terminalCompat` | Terminfo for modern terminals (kitty, alacritty) + headless tools (curl, wget, unzip) |

Platform variants exist for: base (Darwin, HM), operators (Darwin), backup (Darwin), impermanence (HM), home-manager (Darwin).

## Operators

The operators scope manages user accounts declaratively. One operator is designated `primaryUser` - the identity anchor for Home Manager, secrets, and impermanence paths.

Each operator (`users.<name>`) supports:

- `isAdmin` - adds wheel group (sudo access)
- `sshAuthorizedKeys` - SSH public keys for authorized_keys
- `shell` - login shell (default: bash)
- `homeManager.enable` - apply the profile's HM stack to this operator
- `hashedPassword` / `hashedPasswordFile` - password authentication
- `extraGroups` - additional groups on top of roleGroups

Top-level options:

- `primaryUser` - identity anchor (auto-detected when only one operator exists)
- `roleGroups` - groups added to all operators (set by roles, e.g. workstation adds networkmanager/video/audio/docker)
- `rootSshKeys` - root SSH access, independent of operator accounts
- `mutableUsers` - allow imperative passwd changes (default: false)

```nix
nixfleet.operators = {
  primaryUser = "alice";
  users.alice = {
    isAdmin = true;
    sshAuthorizedKeys = [ "ssh-ed25519 AAAA... alice@workstation" ];
    homeManager.enable = true;
    shell = pkgs.zsh;
  };
  users.bob = {
    sshAuthorizedKeys = [ "ssh-ed25519 BBBB... bob@laptop" ];
  };
  rootSshKeys = config.nixfleet.operators._adminSshKeys;
};
```

## Roles

Roles compose scopes with sensible defaults. Import one role per host.

| Role | Type | Scopes imported | Key defaults |
|------|------|----------------|-------------|
| server | Headless | base, operators, firewall, secrets, monitoring, impermanence, o11y, generation-label, terminal-compat, hardware | Firewall on, secrets on, monitoring on, o11y metrics on, no user key, no roleGroups |
| workstation | Interactive | base, operators, firewall, secrets, home-manager, backup, impermanence, o11y, generation-label, terminal-compat, hardware | Firewall on, secrets on, HM on, o11y metrics on, zram swap, roleGroups: networkmanager/video/audio/docker |
| endpoint | Locked-down | base, operators, secrets, impermanence | Secrets on with user key enabled. Consumer provides firewall, HM, and hardware. |
| microvm-guest | VM guest | base, operators, impermanence | Minimal - host owns firewall, backup, and networking |

## Disk Templates

Pre-built disko configurations for common partition layouts.

| Template | Boot | Filesystem | Impermanence |
|----------|------|-----------|-------------|
| btrfs | UEFI | btrfs | No |
| btrfs-bios | Legacy BIOS | btrfs | No |
| btrfs-impermanence | UEFI | btrfs | Yes |
| btrfs-impermanence-bios | Legacy BIOS | btrfs | Yes |
| ext4 | UEFI | ext4 | No |
| luks-btrfs-impermanence | UEFI | LUKS + btrfs | Yes |

Access via `inputs.nixfleet-scopes.scopes.disk-templates.<name>`.

## What Belongs Where

| Content | Belongs in |
|---------|-----------|
| Framework API (mkHost) | nixfleet |
| Service modules (agent, CP, cache, microvm) | nixfleet |
| Infrastructure scopes and roles | [nixfleet-scopes](https://github.com/arcanesys/nixfleet-scopes) |
| Disk templates | [nixfleet-scopes](https://github.com/arcanesys/nixfleet-scopes) |
| Compliance controls and frameworks | [nixfleet-compliance](https://github.com/arcanesys/nixfleet-compliance) |
| Opinionated fleet scopes (dev, graphical, theming) | Your fleet repo |
| Hardware configs and dotfiles | Your fleet repo |
