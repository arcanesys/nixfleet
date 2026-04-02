# Secrets Management

How sensitive data is handled without storing it in plain text.

## The Problem

System configs need secrets: SSH keys, passwords, WiFi credentials. But Nix store paths are world-readable -- you cannot put secrets there.

## Framework Approach

NixFleet is secrets-agnostic. The framework provides extension points (`hostSpec.secretsPath`, `hostSpec.hashedPasswordFile`, etc.) but does not mandate a specific tool. Common choices:

- [agenix](https://github.com/ryantm/agenix) -- age-encrypted secrets, SSH key decryption
- [sops-nix](https://github.com/Mic92/sops-nix) -- multi-format encrypted secrets
- [Vault](https://www.vaultproject.io/) -- centralized secret management

## Typical Pattern (agenix)

1. Secrets are encrypted with age using SSH public keys
2. Encrypted files live in a private secrets repository
3. At boot, agenix decrypts secrets using a provisioned key
4. Decrypted secrets are placed in ephemeral locations

## Integration with Impermanence

Secrets work naturally with ephemeral roots:
- The decryption key is in the persist partition
- Decrypted secrets are ephemeral -- recreated each boot
- No risk of stale secrets accumulating

## Wiring Secrets in Your Fleet

Your fleet repo imports the secrets tool's NixOS module and configures it via `mkHost` modules:

```nix
# Example: agenix in fleet modules
fleetModules = [
  inputs.agenix.nixosModules.default
  ./modules/secrets.nix  # your secret path definitions
];
```

The framework's `hostSpec.secretsPath` option provides a hint for where secrets live, but the actual wiring is fleet-specific.

## Further Reading

- [Technical Secrets Details](../../secrets/README.md) -- paths, keys, bootstrap
- [Installation](../getting-started/installation.md) -- how keys are provisioned
