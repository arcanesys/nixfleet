# Secrets Options

> This module is provided by [nixfleet-scopes](https://github.com/arcanesys/nixfleet-scopes). It is documented here as part of the NixFleet ecosystem reference.

All options under `nixfleet.secrets`. The module is auto-included by mkHost and disabled by default. Enable with `nixfleet.secrets.enable = true`.

## Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | `bool` | `false` | Enable NixFleet secrets wiring (identity paths, persist, boot ordering). |
| `identityPaths.hostKey` | `nullOr str` | `"/etc/ssh/ssh_host_ed25519_key"` | Primary decryption identity (host SSH key). Used on all hosts. |
| `identityPaths.userKey` | `nullOr str` | `"<home>/.keys/id_ed25519"` | Fallback decryption identity (user key). Used on workstations only. Computed from `hostSpec.home`. |
| `identityPaths.enableUserKey` | `bool` | `true` | Include user key in resolved paths. The server role overrides this to `false`. |
| `identityPaths.extra` | `listOf str` | `[]` | Additional identity paths appended to the resolved list. |
| `resolvedIdentityPaths` | `listOf str` | *(computed)* | Read-only. Computed identity paths. Fleet modules pass this to agenix/sops. |

### resolvedIdentityPaths computation

The computed list is:

1. `hostKey` (if non-null)
2. `userKey` (if `enableUserKey` is true and `userKey` is non-null)
3. Each entry in `extra`

`resolvedIdentityPaths` is always computed, even when the scope is disabled, so fleet modules can read it without requiring `nixfleet.secrets.enable`.

## Systemd service

When `enable = true` and `identityPaths.hostKey` is non-null:

| Setting | Value |
|---------|-------|
| Unit | `nixfleet-host-key-check.service` |
| Type | `oneshot` |
| WantedBy | `multi-user.target` |
| Before | `sshd.service` |
| Condition | `ConditionPathExists = !<hostKey>` (runs only if key is missing) |
| Action | Generates ed25519 SSH key at `identityPaths.hostKey` |

A non-fatal activation script (`nixfleet-secrets-check`) warns at activation if any identity path is missing.

## Impermanence

On impermanent hosts (`nixfleet.impermanence.enable = true`), the scope automatically adds to `environment.persistence."/persist"`:
- `files`: `hostKey` and `hostKey.pub`
- `directories`: parent directory of `userKey` (when `enableUserKey` is true)

## Example

```nix
{config, ...}: {
  nixfleet.secrets = {
    enable = true;
    # Defaults are sufficient for most hosts.
    # Servers: resolvedIdentityPaths = ["/etc/ssh/ssh_host_ed25519_key"]
    # Workstations: resolvedIdentityPaths = ["/etc/ssh/ssh_host_ed25519_key" "~/.keys/id_ed25519"]
  };

  # Wire to agenix
  age.identityPaths = config.nixfleet.secrets.resolvedIdentityPaths;
}
```

To add a hardware security key as an extra identity:

```nix
nixfleet.secrets.identityPaths.extra = ["/run/user/1000/gnupg/S.gpg-agent.ssh"];
```
