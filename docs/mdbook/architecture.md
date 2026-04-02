# Architecture

## Purpose

NixFleet is a framework providing `mkHost` -- a single function that returns standard `nixosSystem`/`darwinSystem`. It injects core modules, scopes, and service modules. Fleet repos call `mkHost` and pass their own modules. Opinionated modules (fleet scopes, wrappers, HM programs, config files) belong in consuming fleet repos.

## Location

- `flake.nix` -- entry point
- `modules/` -- all configuration lives here

## Flake Foundation

The flake is built on two key integrations:

- **flake-parts** -- NixOS module system at the flake level. `flake.nix` calls `inputs.flake-parts.lib.mkFlake` which provides `perSystem`, `flake.modules`, and other structured options.
- **import-tree** -- auto-imports every `.nix` file under `modules/` as a flake-parts module. No manual import lists needed.

```nix
outputs = inputs:
  inputs.flake-parts.lib.mkFlake {inherit inputs;} (
    (inputs.import-tree ./modules) // {
      systems = ["x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin"];
    }
  );
```

## The `_` Prefix Convention

Files and directories prefixed with `_` are excluded from import-tree auto-import. They are pulled in via explicit `imports` or relative paths:

| Directory | Contains | Imported by |
|-----------|----------|-------------|
| `_shared/` | `mk-host.nix`, `host-spec-module.nix`, disk templates | `fleet.nix`, mkHost |
| `_hardware/` | Per-host disk-config, hardware-configuration | mkHost modules |

Fleet repos typically add `_config/` (tool configs) and `core/_home/` (HM fragments) -- these are outside the framework.

## mkHost API

`mkHost` is the single public API. It is a closure that binds the framework's flake inputs, then returns `nixosSystem` or `darwinSystem`:

```nix
nixfleet.lib.mkHost {
  hostName = "my-host";
  platform = "x86_64-linux";
  stateVersion = "24.11";  # optional, defaults to "24.11"
  hostSpec = { isDev = true; isImpermanent = true; };
  modules = [ ./hardware/my-host ./my-custom-module.nix ];
  isVm = false;  # optional, for test VMs
}
```

`mkHost` detects the platform and calls the appropriate constructor internally. It injects:
- hostSpec module (binds hostSpec values with `mkDefault`)
- core/_nixos.nix or core/_darwin.nix
- scopes/_base.nix, scopes/_impermanence.nix
- services: _agent.nix, _control-plane.nix
- disko + impermanence modules (NixOS only)
- home-manager module
- user-provided modules

## Fleet Composition

Hosts are declared in `flake.nix` (or `modules/fleet.nix` for the framework test fleet) using `mkHost`:

```nix
let
  mkHost = nixfleet.lib.mkHost;
  orgDefaults = { userName = "admin"; timeZone = "Europe/Paris"; };
in {
  nixosConfigurations.my-host = mkHost {
    hostName = "my-host";
    platform = "x86_64-linux";
    hostSpec = orgDefaults // { isImpermanent = true; };
    modules = [ ./hardware/my-host ];
  };
}
```

Org defaults are plain `let` bindings merged via `//`. Host values override org defaults.

### Framework test fleet

`modules/fleet.nix` contains a minimal test fleet for the framework's own CI. These hosts exist to make eval tests and VM tests pass -- they are not a real org fleet. 5 test hosts exercise different hostSpec flag combinations.

## Scope Self-Activation

Scope modules are plain NixOS/Darwin modules that use `lib.mkIf hS.<flag>` to self-activate. They are imported by `mkHost` directly. Adding a new scope file and importing it in `mkHost` automatically applies to all hosts with the matching flag.

## Framework Inputs

mkHost passes framework inputs (nixpkgs, home-manager, disko, etc.) to modules via `specialArgs = { inherit inputs; }`. Fleet repos access these as the `inputs` argument in their modules. Fleet-specific customization uses hostSpec extensions and plain NixOS modules, not a separate input namespace.

## Key Integrations

| Input | Purpose |
|-------|---------|
| nixpkgs (unstable) | Package set |
| home-manager | User environment |
| nix-darwin | macOS system config |
| disko | Declarative disk partitioning |
| impermanence | Ephemeral root filesystem |
| agenix | Age-encrypted secrets (framework-agnostic, wired via hostSpec) |
| flake-parts | Module system at flake level |
| import-tree | Auto-import all `.nix` files under `modules/` |
| nixos-anywhere | Remote NixOS installation |
| nixos-hardware | Hardware configuration modules |
| lanzaboote | Secure Boot support |
| treefmt-nix | Multi-language formatting (alejandra + shfmt) |

## Dependencies

- Host files depend on `_shared/mk-host.nix` and `_shared/host-spec-module.nix`
- `core/nixos.nix` and `core/darwin.nix` depend on `hostSpec` options from `host-spec-module.nix`
- Scopes are plain modules imported by mkHost

## Links

- [Host System](hosts/README.md)
- [Scope System](scopes/README.md)
- [Core Modules](core/README.md)
