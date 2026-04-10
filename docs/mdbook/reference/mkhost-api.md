# mkHost API

## Parameters

```nix
nixfleet.lib.mkHost {
  hostName    = "myhost";
  platform    = "x86_64-linux";
  stateVersion = "24.11";       # optional
  hostSpec    = { ... };         # optional
  modules     = [ ... ];         # optional
  isVm        = false;           # optional
}
```

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `hostName` | string | yes | -- | Machine hostname. Injected into `hostSpec.hostName` and `networking.hostName`. |
| `platform` | string | yes | -- | Target platform: `x86_64-linux`, `aarch64-linux`, `aarch64-darwin`, `x86_64-darwin`. |
| `stateVersion` | string | no | `"24.11"` | NixOS or nix-darwin state version. |
| `hostSpec` | attrset | no | `{}` | Host configuration flags. Values are set with `lib.mkDefault` (overridable by modules). `hostName` is always forced to match the parameter. |
| `modules` | list | no | `[]` | Additional NixOS or Darwin modules appended after framework modules. |
| `isVm` | bool | no | `false` | Inject QEMU VM hardware config (virtio disk, SPICE, DHCP, software GL). |

## Return type

- **Linux platforms** (`x86_64-linux`, `aarch64-linux`): Returns the result of `nixpkgs.lib.nixosSystem`.
- **Darwin platforms** (`aarch64-darwin`, `x86_64-darwin`): Returns the result of `darwin.lib.darwinSystem`.

Platform detection is automatic based on `platform`.

## Injected modules

### NixOS (Linux)

mkHost injects these modules before user-provided `modules`:

1. `nixpkgs.hostPlatform` setting
2. hostSpec module + effective hostSpec values
3. disko NixOS module
4. impermanence NixOS module
5. Core NixOS module (`_nixos.nix`)
6. Base scope (NixOS part)
7. Impermanence scope (NixOS part)
8. Agent service module (disabled by default)
9. Control plane service module (disabled by default)
10. Cache server service module — harmonia (disabled by default)
11. Cache client service module — generic substituter config (disabled by default)
12. Home Manager NixOS module with user config

When `isVm = true`, additionally injects:
- QEMU disk and hardware config
- SPICE agent, forced DHCP, software GL

### Darwin (macOS)

mkHost injects these modules before user-provided `modules`:

1. `nixpkgs.hostPlatform` setting
2. hostSpec module + effective hostSpec values (with `isDarwin = true`)
3. Core Darwin module (`_darwin.nix`)
4. Base scope (Darwin part)
5. Home Manager Darwin module with user config

## Home Manager integration

mkHost configures Home Manager for both platforms:

| Setting | Value |
|---------|-------|
| `useGlobalPkgs` | `true` |
| `useUserPackages` | `true` (NixOS only) |
| `backupCommand` | Moves conflicting files to `.nbkp.<timestamp>`, keeps 5 most recent |
| User home directory | `/home/<userName>` (Linux), `/Users/<userName>` (Darwin) |
| `enableNixpkgsReleaseCheck` | `false` |
| `systemd.user.startServices` | `"sd-switch"` (NixOS only) |

HM receives the hostSpec module and effective hostSpec values, plus framework HM modules (base, impermanence on Linux).

## Framework inputs

Framework inputs are passed via `specialArgs = {inherit inputs;}`. Modules can access them as the `inputs` argument. These are NixFleet's own inputs (nixpkgs, home-manager, disko, etc.), not fleet-level inputs.

## Exports

All exports from the NixFleet flake:

| Export | Path | Description |
|--------|------|-------------|
| `lib.mkHost` | `nixfleet.lib.mkHost` | Host definition function |
| `lib.mkVmApps` | `nixfleet.lib.mkVmApps` | VM helper apps generator |
| `nixosModules.nixfleet-core` | `nixfleet.nixosModules.nixfleet-core` | Raw core NixOS module (without mkHost) |
| `diskoTemplates.btrfs` | `nixfleet.diskoTemplates.btrfs` | Standard btrfs disk layout |
| `diskoTemplates.btrfs-impermanence` | `nixfleet.diskoTemplates.btrfs-impermanence` | Btrfs layout with impermanence subvolumes |
| `templates.default` / `templates.standalone` | `nix flake init -t nixfleet` | Single-host template |
| `templates.fleet` | `nix flake init -t nixfleet#fleet` | Multi-host fleet template |
| `templates.batch` | `nix flake init -t nixfleet#batch` | Batch hosts template |
| `packages.<system>.iso` | `nix build .#iso` | Custom installer ISO |
| `packages.<system>.nixfleet-agent` | -- | Rust agent binary |
| `packages.<system>.nixfleet-control-plane` | -- | Rust control plane binary |
| `packages.<system>.nixfleet-cli` | -- | Rust CLI binary |
| `flakeModules.apps` | -- | VM apps and validate (for fleet repos) |
| `flakeModules.tests` | -- | Eval and VM tests (for fleet repos) |
| `flakeModules.iso` | -- | ISO builder (for fleet repos) |
| `flakeModules.formatter` | -- | Alejandra + shfmt formatter (for fleet repos) |
