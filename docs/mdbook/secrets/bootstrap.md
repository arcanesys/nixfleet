# Secrets Bootstrap

## Purpose

During NixOS installation, the secrets decryption key must be provisioned to the target machine before secrets can be decrypted. This is typically handled via nixos-anywhere's `--extra-files`.

## Bootstrap Flow (agenix example)

1. The install process looks for a decryption key (e.g., `~/.keys/id_ed25519`)
2. Creates a temp directory structure: `<tmp>/persist/home/<user>/.keys/id_ed25519`
3. Passes to nixos-anywhere via `--extra-files <tmp>`
4. nixos-anywhere copies the key to the target's persist partition during installation
5. On first boot, the secrets tool finds the key and decrypts all secrets

## Key Locations (typical)

| Path | Purpose | Persisted |
|------|---------|-----------|
| `~/.keys/id_ed25519` | Decryption key | Yes (impermanence) |
| `~/.ssh/id_ed25519` | Runtime SSH key (secrets-managed) | No (ephemeral) |
| `/persist/home/<user>/.keys/` | Persist bind mount source | Yes |

## Security Notes

- The decryption key is copied from the installer's machine to the target
- On impermanent hosts, an activation script ensures correct ownership of the keys directory

## Links

- [Secrets Overview](README.md)
- [Impermanence](../scopes/impermanence.md)
