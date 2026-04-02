# Impermanence

Why the root filesystem wipes on every boot — and why that is a good thing.

## The Concept

On impermanent hosts, the root filesystem (`/`) is ephemeral. Every boot starts fresh. Only explicitly persisted paths survive across reboots.

This means:
- **No configuration drift** — the system always matches the Nix config
- **No accumulated cruft** — temp files, caches, leftover configs vanish
- **Forced explicitness** — if something needs to persist, you declare it

## How It Works

1. The root partition uses btrfs with a subvolume that gets wiped on boot
2. A `/persist` partition holds data that must survive reboots
3. The `impermanence` module creates bind mounts from `/persist` to their expected locations
4. Programs see their data at normal paths (e.g., `~/.local/share/`) without knowing it is a bind mount

## What Persists

Persist paths are declared alongside the programs that need them:

- SSH `known_hosts` (not the full `.ssh` directory — keys come from agenix)
- Browser profiles
- Docker data
- WiFi connections
- Application state (VS Code, Zed, etc.)

## What Does Not Persist

- `/tmp`, `/var/tmp` — ephemeral by nature
- `.ssh`, `.gnupg` directories — recreated each boot by agenix and Home Manager
- Downloaded files outside persisted paths
- System logs (unless configured otherwise)

## Opting In

Impermanence is a per-host flag:

```nix
hostSpec = {
  isImpermanent = true;
};
```

Hosts without this flag use a normal persistent root.

## Further Reading

- [Technical Impermanence Details](../../scopes/impermanence.md) — paths and implementation
- [Secrets Management](secrets.md) — how secrets work with ephemeral roots
