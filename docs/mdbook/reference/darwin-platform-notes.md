# Darwin Platform Quirks

Differences between NixOS and nix-darwin that affect nixfleet. This document captures non-obvious behaviors discovered during Darwin fleet integration.

## networking.hostName is null

**NixOS:** `config.networking.hostName` is always set (from the NixOS config).

**Darwin:** `config.networking.hostName` is `null` by default. nix-darwin uses the system hostname but doesn't populate this option.

**Fix:** Use `config.hostSpec.hostName` (set by mkHost) everywhere instead of `config.networking.hostName`. This works on both platforms.

## Activation scripts API differs

**NixOS:** `system.activationScripts.<name>.text = "..."` — arbitrary named scripts.

**Darwin:** `system.activationScripts.preActivation.text` and `system.activationScripts.postActivation.text` — only two hooks, no arbitrary names.

**Fix:** Use `preActivation` or `postActivation` on Darwin. When a module is shared between platforms, use `lib.mkMerge` with `lib.mkIf isDarwin` to conditionally define the Darwin-only option without breaking NixOS evaluation.

## /run/current-system exists on Darwin

**NixOS:** `/run/current-system` is a symlink to the active store path, created by `switch-to-configuration`.

**Darwin:** `/run/current-system` also exists — created by the `activate` script (line: `ln -sfn "$(readlink -f "$systemConfig")" /run/current-system`). It's a direct symlink to the store path, same as NixOS.

**Implication:** The agent can use `/run/current-system` on both platforms for generation detection. No platform-specific path needed.

## /nix/var/nix/profiles/system is a two-level symlink

**Both platforms:** `/nix/var/nix/profiles/system` → `system-N-link` → `/nix/store/<hash>-...-system-...`

**Implication:** `read_link()` only resolves one level (returns `system-N-link`). Use `canonicalize()` to fully resolve, or use `/run/current-system` which is a single-level symlink on both platforms.

## Activation mechanism

**NixOS:** `<store_path>/bin/switch-to-configuration switch` — a Rust binary that manages systemd units, /etc, bootloader, etc. Uses `flock` on `/run/nixos/switch-to-configuration.lock`.

**Darwin:** `<store_path>/activate` — a bash script (~900 lines) that manages launchd plists, /etc files, system defaults, user activation. No lock file.

**darwin-rebuild switch** does three things in order:
1. `nix-env -p /nix/var/nix/profiles/system --set "$systemConfig"` (profile update)
2. `$systemConfig/activate-user` (legacy user activation, if present)
3. `$systemConfig/activate` (system activation)

**Agent fire_switch order:** Profile update first, then activate. This matches `darwin-rebuild switch`.

**SSH deploy order:** Same — `nix-env --set` then `activate`. Not `switch-to-configuration`.

## Launchd vs systemd for the agent

**NixOS:** `systemd.services.nixfleet-agent` — Type=simple, Restart=always, RestartSec=30.

**Darwin:** `launchd.daemons.nixfleet-agent` — KeepAlive=true, RunAtLoad=true. Plist at `/Library/LaunchDaemons/com.nixfleet.agent.plist`.

**Key differences:**
- Launchd auto-restarts crashed daemons (KeepAlive) — no explicit restart config needed
- Logs go to `/var/log/nixfleet-agent.log` (StandardOutPath/StandardErrorPath) not journald
- State directory (`/var/lib/nixfleet`) must be created via `preActivation` script, not systemd's `StateDirectory`
- WorkingDirectory must exist before the daemon starts or launchd returns I/O error (exit 5)

## Health checks: launchd vs systemd

**NixOS:** `healthChecks.systemd` — checks `systemctl is-active <unit>`.

**Darwin:** `healthChecks.launchd` — checks `launchctl list <label>`, verifies PID presence in output (loaded-but-stopped services return exit 0 without a PID line).

**Fallback:** NixOS uses `systemctl is-system-running`. Darwin uses `launchctl list` (system responsive check).

## Custom CA trust

**NixOS:** `security.pki.certificateFiles` adds CAs to the system trust store. The agent's reqwest/rustls uses native roots, which includes these.

**Darwin:** `security.pki.certificateFiles` doesn't exist in nix-darwin. The macOS keychain is the system trust store, managed by `security add-trusted-cert`.

**Fix:** Added `--ca-cert` flag to the agent. Passes a PEM file as an additional root certificate to reqwest. Works on both platforms without touching the system trust store.

## Determinate Nix on Darwin

**Standard Nix:** nix-darwin manages `/etc/nix/nix.conf` and writes `/etc/nix/machines` for `nix.buildMachines`.

**Determinate Nix:** Owns `/etc/nix/nix.conf` (`# do not modify! this file will be replaced!`). Uses `!include nix.custom.conf` for user settings. nix-darwin's `nix.buildMachines` evaluates correctly but has **no effect** — the generated nix.conf is overwritten by Determinate Nix.

**Fix:** Write builders to `/etc/nix/machines` directly via `postActivation`, then set `builders = @/etc/nix/machines` in `/etc/nix/nix.custom.conf`. Restart the nix daemon after activation (`sudo launchctl kickstart -k system/systems.determinate.nix-daemon`).

## Remote builder SSH

The nix daemon runs as root on both platforms. For remote builders:
- Root must have an SSH key that the builder accepts
- The builder's host key must be in the system known_hosts (`/etc/ssh/ssh_known_hosts`)
- On Darwin, the nix daemon restart is required to pick up new builder config (it reads nix.conf only at startup)

## CLI paths on macOS

| Purpose | Linux | macOS |
|---------|-------|-------|
| Config | `.nixfleet.toml` (CWD walk) | Same |
| Credentials | `~/.config/nixfleet/credentials.toml` | `~/Library/Application Support/nixfleet/credentials.toml` |
| Operation logs | `~/.local/state/nixfleet/logs/` | `~/Library/Logs/nixfleet/` |

## Test sandbox differences

**NixOS build sandbox:** `$HOME` writable, `XDG_CONFIG_HOME` respected by `dirs::config_dir()`.

**Darwin build sandbox:** `$HOME=/homeless-shelter` (read-only). `dirs::config_dir()` ignores `XDG_CONFIG_HOME` and uses `~/Library/Application Support`. Tests that write to config dirs must set `HOME` to a temp dir.
