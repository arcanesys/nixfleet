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
| `hostName` | string | yes | -- | Machine hostname. Forced into `hostSpec.hostName` (not overridable). |
| `platform` | string | yes | -- | Target platform: `x86_64-linux`, `aarch64-linux`, `aarch64-darwin`, `x86_64-darwin`. |
| `stateVersion` | string | no | `"24.11"` | NixOS state version (set with `lib.mkDefault`). Not used for Darwin - consumers set it in their host modules. |
| `hostSpec` | attrset | no | `{}` | Host configuration flags. Values are set with `lib.mkDefault` (overridable by modules). `hostName` is always forced to match the parameter. |
| `modules` | list | no | `[]` | Additional NixOS or Darwin modules appended after framework modules. |
| `isVm` | bool | no | `false` | Inject QEMU VM hardware config (virtio disk, SPICE, DHCP, software GL). NixOS only. |

## Return type

- **Linux platforms** (`x86_64-linux`, `aarch64-linux`): Returns the result of `nixpkgs.lib.nixosSystem`.
- **Darwin platforms** (`aarch64-darwin`, `x86_64-darwin`): Returns the result of `darwin.lib.darwinSystem`.

Platform detection is automatic based on `platform`.

## Injected modules

mkHost injects framework modules before user-provided `modules`. These are mechanism-only - no opinions about packages, services, or user environment.

### NixOS (Linux)

1. `system.stateVersion` (mkDefault)
2. `nixpkgs.hostPlatform` set to `platform`
3. hostSpec module (option declarations)
4. hostSpec values set with `lib.mkDefault` (overridable by consumer modules)
5. `hostSpec.hostName` forced to match the `hostName` parameter
6. Impermanence scope from nixfleet-scopes (declares options only - inert unless `nixfleet.impermanence.enable = true`)
7. Core NixOS module (`_nixos.nix`)
8. Agent service module (disabled by default)
9. Control plane service module (disabled by default)
10. Cache server service module (disabled by default)
11. Cache client module (disabled by default)
12. MicroVM host module (disabled by default)
13. User-provided `modules`

When `isVm = true`, additionally injects:
- QEMU disk config and hardware configuration
- SPICE agent (`services.spice-vdagentd.enable`)
- Forced DHCP (`networking.useDHCP = lib.mkForce true`)
- Software GL (`LIBGL_ALWAYS_SOFTWARE`, mesa)

**Why impermanence is auto-imported:** NixFleet's internal service modules (agent, control-plane, microvm-host) conditionally contribute to `environment.persistence`. The NixOS module system validates option paths even inside `lib.mkIf false`, so the impermanence scope must be present to declare those options. The scope is inert (zero cost) until explicitly enabled.

### Darwin (macOS)

1. `nixpkgs.hostPlatform` set to `platform`
2. hostSpec module (option declarations)
3. hostSpec values set with `lib.mkDefault` (overridable by consumer modules)
4. `hostSpec.hostName` forced to match the `hostName` parameter
5. `hostSpec.isDarwin = true`
6. Core Darwin module (`_darwin.nix`)
7. Agent Darwin module (disabled by default)
8. User-provided `modules`

### NOT auto-included

These are consumer responsibilities - import them via roles or explicitly in `modules`:

- **disko** - disk partitioning (import from nixfleet-scopes or use `diskoTemplates`)
- **base scope** - opinionated system defaults (import from nixfleet-scopes)
- **home-manager** - user environment management (import from nixfleet-scopes)
- **operators scope** - multi-user inventory (import from nixfleet-scopes)
- **All other infrastructure scopes** - firewall, secrets, backup, monitoring, etc.

The typical pattern is to import a role, which bundles the relevant scopes:

```nix
modules = [
  inputs.nixfleet.scopes.roles.workstation  # includes base, HM, operators, etc.
  ./hardware-configuration.nix
];
```

## Framework inputs

Framework inputs are passed via `specialArgs = {inherit inputs;}`. Modules can access them as the `inputs` argument. These are NixFleet's own inputs (nixpkgs, home-manager, disko, impermanence, etc.), not fleet-level inputs.

## Home Manager

Home Manager is a scope from nixfleet-scopes. It is **not** auto-injected by mkHost.

Import it via a role (workstation and endpoint roles include it) or manually:

```nix
modules = [
  nixfleet.scopes.home-manager
  { nixfleet.home-manager.enable = true; }
];
```

The scope fans out `profileImports` to all operators with `homeManager.enable = true`.

## Scope re-exports

NixFleet re-exports nixfleet-scopes so consumers do not need a separate flake input:

```nix
# These are equivalent:
inputs.nixfleet-scopes.scopes.roles.workstation
inputs.nixfleet.scopes.roles.workstation
```

Available under `inputs.nixfleet.scopes`:
- `scopes.roles.*` - workstation, server, endpoint, microvm-guest
- `scopes.base` - opinionated system defaults
- `scopes.home-manager` - HM integration
- `scopes.impermanence` - impermanence support
- `scopes.disk-templates.*` - disko disk layouts
- All other nixfleet-scopes exports

## Exports

All exports from the NixFleet flake:

| Export | Access path | Description |
|--------|------------|-------------|
| `lib.mkHost` | `inputs.nixfleet.lib.mkHost` | Host definition function |
| `lib.mkVmApps` | `inputs.nixfleet.lib.mkVmApps` | VM helper apps generator |
| `nixosModules.nixfleet-core` | `inputs.nixfleet.nixosModules.nixfleet-core` | Raw core NixOS module (without mkHost) |
| `scopes` | `inputs.nixfleet.scopes` | Re-export of nixfleet-scopes (no separate input needed) |
| `diskoTemplates` | `inputs.nixfleet.diskoTemplates` | Alias for `scopes.disk-templates` |
| `flakeModules.apps` | `inputs.nixfleet.flakeModules.apps` | VM lifecycle apps (for fleet repos) |
| `flakeModules.tests` | `inputs.nixfleet.flakeModules.tests` | Eval and VM test infrastructure (for fleet repos) |
| `flakeModules.iso` | `inputs.nixfleet.flakeModules.iso` | ISO builder (for fleet repos) |
| `flakeModules.formatter` | `inputs.nixfleet.flakeModules.formatter` | Treefmt config - alejandra + shfmt (for fleet repos) |
| `templates.default` | `nix flake init -t nixfleet` | Single-host template (same as standalone) |
| `templates.standalone` | `nix flake init -t nixfleet#standalone` | Single NixOS machine |
| `templates.batch` | `nix flake init -t nixfleet#batch` | Batch of identical hosts from a template |
| `templates.fleet` | `nix flake init -t nixfleet#fleet` | Multi-host fleet with flake-parts |
