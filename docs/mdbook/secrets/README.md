# Secrets Management

## Framework Approach

NixFleet is **secrets-agnostic**. The framework does not bundle any secrets tool -- it provides clean extension points via `hostSpec` options for consuming fleet repos to plug in their secrets management of choice.

The framework test fleet has no secrets at all (`hashedPasswordFile = null`).

## Extension Points

| Mechanism | Location | Purpose |
|-----------|----------|---------|
| `hostSpec.secretsPath` | `host-spec-module.nix` | Pass secrets repo path to modules without hardcoding |
| `hostSpec.hashedPasswordFile` | `host-spec-module.nix` | Wired to `users.users.<name>.hashedPasswordFile` |
| `hostSpec.rootHashedPasswordFile` | `host-spec-module.nix` | Wired to `users.users.root.hashedPasswordFile` |

## Wiring Secrets in a Fleet

Consuming fleet repos import their chosen secrets tool and wire it via `mkHost` modules:

```nix
# Example: agenix integration in a fleet repo
fleetModules = [
  inputs.agenix.nixosModules.default
  ./modules/org-secrets.nix  # defines secret file paths
];
```

The secrets module defines encrypted file paths, decryption identity paths, and output locations. The framework's `hostSpec` options provide the connection points.

## Updating Secrets

When using a secrets repo as a flake input:

```sh
nix flake update secrets
sudo nixos-rebuild switch --flake .#<hostname>
```

## Links

- [Bootstrap](bootstrap.md)
- [WiFi](wifi.md)
- [Impermanence scope](../scopes/impermanence.md)
