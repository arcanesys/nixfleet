# Sécurix endpoint example

Demonstrates running [Sécurix](https://github.com/cloud-gouv/securix) — an
ANSSI-hardened NixOS distribution for government admin laptops — under
`nixfleet.lib.mkHost`.

## What this shows

- Consuming `securix.nixosModules.securix-base` and `securix.nixosModules.securix-hardware.t14g6` as regular NixOS modules.
- Using NixFleet's endpoint escape-hatches (`managedUser = false`,
  `enableHomeManager = false`, `customFilesystems = true`,
  `skipDefaultFirewall = true`) so Sécurix's own modules are authoritative
  for user management, filesystem layout, and firewall.
- Composing Sécurix's companion inputs (lanzaboote, agenix, disko) alongside
  `securix-base` — these are required; `securix-base` does not import them.

## Prerequisites

- Nix with flakes enabled.
- On first use:
  ```
  nix flake lock
  ```
  This fetches all inputs (nixfleet, securix, nixpkgs, disko, lanzaboote, agenix, nixos-hardware, flake-utils). Takes a few minutes.

## Eval (fast)

```
nix eval .#nixosConfigurations.lab-endpoint.config.system.build.toplevel.drvPath
```

Expected output: a `/nix/store/...-nixos-system-lab-endpoint-....drv` path. This confirms the entire module tree — NixFleet + Sécurix + all four escape-hatches — evaluates cleanly.

## Full build (slow)

```
nix build .#nixosConfigurations.lab-endpoint.config.system.build.toplevel
```

## Known workarounds in this example

Several workarounds are encoded in the `modules` block. They are needed because of **upstream Sécurix bugs** discovered during the phase 0/1 pilot. Track their upstream resolution before removing:

1. **`boot.lanzaboote.enable = lib.mkOverride 0 false`** — Sécurix's `modules/bootloader.nix` uses `lib.mkForce true` on `lanzaboote.enable`. In a VM without Secure Boot hardware, we need `mkOverride 0` (priority 0) to defeat `mkForce` (priority 50). A cleaner path is to submit a PR upstream relaxing Sécurix's `mkForce` to `mkDefault` on the lanzaboote assignment.

2. **`securix.self.user.username = "operator"`** — setting only `.email` triggers a latent bug in Sécurix's `deriveUsernameFromEmail`: emails with dotted domains (e.g. `name@gouv.fr`) embed the `@` sign in the derived username, which fails NixOS's user-name type check. Setting `.username` explicitly bypasses this.

3. **`securix.graphical-interface.variant = lib.mkDefault "kde"`** — even when `graphical-interface.enable = false`, Sécurix's module evaluates the `variant` enum unconditionally. Any valid value works; `"kde"` is an arbitrary pick.

4. **`users.allowNoPasswordLogin = true`** — Sécurix's default user carries `hashedPassword = "!"` (intentionally locked). Without a real password file or SSH key, NixOS flags this as a lockout. This is a smoke-test affordance; real deployments supply real auth.

5. **Companion inputs are re-exported** via `securix.inputs.*`. This example imports lanzaboote, agenix, and disko from Sécurix's own flake inputs (via `securix.inputs.lanzaboote.nixosModules.lanzaboote`, etc.) so versions stay consistent with Sécurix's expectations.

## Real-hardware deployment

For production use on a real endpoint (not a VM):

- Flip `isVm = false` in `flake.nix`.
- Set `securix.self.mainDisk = "/dev/nvme0n1"` (or whatever the target disk is).
- Remove the VM `fileSystems."/"` override — Sécurix's disko module provides the layout.
- Re-enable lanzaboote: remove the `boot.lanzaboote.enable` and `boot.loader.systemd-boot.enable` overrides. Ensure the target has Secure Boot and a TPM.
- Replace `users.allowNoPasswordLogin = true` with real auth — at minimum, set `securix.self.user.hashedPassword` to a `mkpasswd -m sha-512` output, or provision SSH keys via `securix.self.user`.
- Deploy using Sécurix's `buildUSBInstallerISO` rather than `nixos-anywhere`: physical install handles Secure Boot key enrollment, TPM2 host-key generation, and age recipient export. See Sécurix's lib for `lib.forSystem <system>.buildUSBInstallerISO`.

## Status

Pilot acceptance artifact. See the phase 0/1 plan in the fleet repo for background:
`docs/superpowers/plans/2026-04-16-securix-nixfleet-phase-0-1.md`.
