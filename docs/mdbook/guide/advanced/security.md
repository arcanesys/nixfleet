# Security Model

How NixFleet handles security across multiple layers.

## Defense in Depth

Security is layered, not centralized:

1. **Encrypted secrets** -- secrets tool encrypts everything sensitive
2. **Ephemeral root** -- impermanent hosts wipe on boot, reducing attack surface
3. **SSH hardening** -- restricted authentication methods, no root login
4. **Firewall** -- default deny inbound

## Secrets Security

- Secrets are never in the Nix store (world-readable)
- Encrypted at rest in a private repository
- Decrypted to ephemeral paths at boot
- Decryption key lives in the persist partition only

## Network Security

- Firewall enabled with default deny
- SSH uses key-only authentication
- No unnecessary ports exposed
- SPICE (for VMs) is localhost-only

## Further Reading

- [Secrets Management](../concepts/secrets.md) -- how secrets work
- [Impermanence](../concepts/impermanence.md) -- ephemeral root filesystem
