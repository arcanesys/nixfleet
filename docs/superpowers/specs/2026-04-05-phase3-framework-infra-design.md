# Phase 3: Framework Infrastructure

## Overview

Four new infrastructure primitives as optional NixOS modules in nixfleet. Each module is disabled by default — zero cost for consumers who don't enable them.

**Scope:** Attic binary cache (server + client), MicroVM host, backup backend enhancement (restic + borgbackup), firewall bridge forwarding for microVMs.

**Not in scope:** Rollout strategies (Rust CP changes — separate PR).

## 1. Attic Binary Cache

### Server (`modules/scopes/nixfleet/_attic-server.nix`)

Wraps `services.atticd` from the `attic` flake input.

**Options under `services.nixfleet-attic-server`:**

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | bool | false | Activate Attic cache server |
| `listen` | str | `"0.0.0.0:8081"` | Listen address:port |
| `openFirewall` | bool | false | Open listen port in firewall |
| `signingKeyFile` | str | required | Path to cache signing key |
| `storage.type` | enum | `"local"` | `"local"` or `"s3"` |
| `storage.local.path` | str | `/var/lib/nixfleet-attic/storage` | Local storage directory |
| `storage.s3.bucket` | str | — | S3 bucket name |
| `storage.s3.region` | str | — | S3 region |
| `storage.s3.endpoint` | str | null | S3-compatible endpoint URL |
| `garbageCollection.schedule` | str | `"weekly"` | Systemd calendar for GC |
| `garbageCollection.keepSinceLastPush` | str | `"90d"` | Duration to keep paths after last push |

**Behavior:**
- Generates `/etc/nixfleet-attic/server.toml` from options
- Systemd service with hardening matching agent/CP pattern
- Impermanence: persists `/var/lib/nixfleet-attic`
- GC runs as a systemd timer calling `atticd --mode garbage-collector-once`

### Client (`modules/scopes/nixfleet/_attic-client.nix`)

Configures hosts to pull from the cache.

**Options under `services.nixfleet-attic-client`:**

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | bool | false | Configure this host as a cache client |
| `cacheUrl` | str | required | URL of the Attic server |
| `publicKey` | str | required | Cache signing public key |

**Behavior:**
- Adds `cacheUrl` to `nix.settings.substituters`
- Adds `publicKey` to `nix.settings.trusted-public-keys`
- Adds `attic-client` package to system packages for manual push/pull

## 2. MicroVM Host (`modules/scopes/nixfleet/_microvm-host.nix`)

Wraps `microvm.host` from the `microvm` flake input.

**Options under `services.nixfleet-microvm-host`:**

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | bool | false | Activate MicroVM host |
| `bridge.name` | str | `"nixfleet-br0"` | Bridge interface name |
| `bridge.address` | str | `"10.42.0.1/24"` | Bridge IP with CIDR |
| `dhcp.enable` | bool | true | Run dnsmasq DHCP on bridge |
| `dhcp.range` | str | `"10.42.0.10,10.42.0.254,1h"` | DHCP range |
| `vms` | attrsOf submodule | `{}` | MicroVM definitions |

**VM submodule options:**

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `flake` | str | required | Flake reference for the VM's NixOS config |
| `vcpu` | int | 1 | Virtual CPU count |
| `mem` | int | 512 | Memory in MiB |
| `macAddress` | str | auto | MAC address (auto-generated from VM name if null) |
| `volumes` | listOf attrs | `[]` | Block device volumes |
| `shares` | listOf attrs | `[]` | virtiofs shared directories |

**Behavior:**
- Creates bridge interface with static IP
- Auto-creates TAP interface per VM, attached to bridge
- Optional dnsmasq DHCP server on bridge
- Enables IP forwarding and NAT for bridge subnet
- MicroVMs are first-class fleet members (if their config enables nixfleet-agent, they report to CP)
- Impermanence: persists `/var/lib/microvms`

## 3. Backup Enhancement (`modules/scopes/_backup.nix`)

Extends existing scaffolding with concrete backends.

**New options:**

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `backend` | enum | null | `null`, `"restic"`, or `"borgbackup"` |
| `restic.repository` | str | — | Restic repo URL (required when backend = restic) |
| `restic.passwordFile` | str | — | Path to repo password file |
| `borgbackup.repository` | str | — | Borg repo path/URL (required when backend = borgbackup) |
| `borgbackup.passphraseFile` | str | null | Path to passphrase file (null = repokey) |
| `borgbackup.encryption` | str | `"repokey"` | Encryption mode |

**Behavior when `backend = "restic"`:**
- Sets `ExecStart` to `restic backup` with `--repo`, `--password-file`, paths, `--exclude` patterns, `--tag hostname`
- Adds prune step: `restic forget --keep-daily N --keep-weekly N --keep-monthly N --prune`
- Adds `restic` package to system packages

**Behavior when `backend = "borgbackup"`:**
- Sets `ExecStart` to `borg create` with archive naming, paths, excludes
- Adds prune step: `borg prune --keep-daily N --keep-weekly N --keep-monthly N`
- Adds `borgbackup` package to system packages

**Behavior when `backend = null`:** unchanged (fleet sets ExecStart).

## 4. Firewall Enhancement (`modules/scopes/_firewall.nix`)

**New:** When `services.nixfleet-microvm-host.enable` on a non-minimal host:
- Allow forwarding on the bridge interface (`iifname "nixfleet-br0" accept` in forward chain)
- NAT masquerade for bridge subnet outbound traffic

Port opening for Attic server is handled by the Attic server module itself (same pattern as agent/CP).

## 5. Flake Changes

**New inputs:**
```nix
attic = {
  url = "github:zhaofengli/attic";
  inputs.nixpkgs.follows = "nixpkgs";
};
microvm = {
  url = "github:astro/microvm.nix";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

**mkHost:** imports `_attic-server.nix`, `_attic-client.nix`, `_microvm-host.nix` into `frameworkNixosModules`.

## 6. Test Hosts

| Host | Purpose |
|------|---------|
| `attic-test` | Attic server (local storage) + client |
| `microvm-test` | MicroVM host with bridge + 1 VM definition |
| `backup-restic-test` | Backup with restic backend |

## 7. Eval Tests

- **Attic server:** listen port in firewall, impermanence paths, signing key in config
- **Attic client:** substituters contain cache URL, trusted-public-keys has public key
- **MicroVM host:** bridge configured, IP forwarding enabled
- **Backup restic:** ExecStart contains `restic backup`, retention flags correct

## Success Criteria

- All existing eval tests still pass
- New eval tests pass for each module
- Each module can be enabled with 3-5 lines of config
- Modules are no-ops when disabled (zero eval cost)
- Documentation updated (CLAUDE.md scope table, structure tree)
