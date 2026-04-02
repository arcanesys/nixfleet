# Host System

## Purpose

All hosts are declared inline in `flake.nix` via `nixfleet.lib.mkHost`. Scope modules auto-activate based on each host's `hostSpec` flags -- hosts never list features manually.

## Location

- `flake.nix` -- host definitions (inline, using `nixfleet.lib.mkHost`)
- `modules/_shared/lib/mk-host.nix` -- `mkHost` implementation
- `modules/_shared/host-spec-module.nix` -- hostSpec option definitions
- `modules/_hardware/<name>/` -- per-host disk-config and hardware-configuration

## Framework Test Fleet

The framework ships a minimal test fleet defined in `modules/fleet.nix`. These hosts exist to make eval tests and VM tests pass -- they are **not** a real fleet. Fleet-specific `hostSpec` options are declared by consuming fleet repos, not the framework.

### Test hosts

| Host | Platform | Flags | Purpose |
|------|----------|-------|---------|
| web-01 | x86_64-linux | `isImpermanent` | Shared defaults / SSH / impermanence / password tests |
| web-02 | x86_64-linux | `isImpermanent` | Scope activation / SSH hardening tests |
| dev-01 | x86_64-linux | `userName=alice` | userName override tests |
| edge-01 | x86_64-linux | `isMinimal` | Minimal host tests |
| srv-01 | x86_64-linux | `isServer` | Server host tests |

> Real hosts are defined in consuming fleet repos, not in the framework.

## hostSpec Framework Options

The framework defines these options in `host-spec-module.nix`:

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `userName` | str | -- | Primary username |
| `hostName` | str | -- | Machine hostname |
| `timeZone` | str | `"UTC"` | IANA timezone |
| `locale` | str | `"en_US.UTF-8"` | System locale |
| `keyboardLayout` | str | `"us"` | XKB layout |
| `home` | str | platform-aware | Home directory (`/home/<user>` on Linux, `/Users/<user>` on Darwin) |
| `sshAuthorizedKeys` | list of str | `[]` | SSH public keys |
| `secretsPath` | str or null | null | Secrets repo path hint |
| `isMinimal` | bool | false | Suppress base packages |
| `isDarwin` | bool | false | macOS host |
| `isImpermanent` | bool | false | Enable impermanence |
| `isServer` | bool | false | Headless server |
| `hashedPasswordFile` | str or null | null | Primary user password file |
| `rootHashedPasswordFile` | str or null | null | Root password file |

Additional flags (e.g., `isDev`, `isGraphical`) are declared by consuming fleet repos.

## Adding a New Host

See the [new host guide](../guide/advanced/new-host.md) for step-by-step instructions. The short version: add a `nixfleet.lib.mkHost` call in `flake.nix`, add hardware config in `_hardware/`, and deploy with `nixos-anywhere` or `nixos-rebuild`.

## Links

- [Architecture](../architecture.md)
