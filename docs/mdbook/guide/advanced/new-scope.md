# Adding a New Scope

How to add a new feature group that self-activates based on host flags.

## 1. Define the Flag

Add a new option in `modules/_shared/host-spec-module.nix` (framework) or in your fleet's host-spec extensions:

```nix
useMyFeature = lib.mkOption {
  type = lib.types.bool;
  default = false;
  description = "Enable my feature";
};
```

## 2. Create the Scope Module

Create a plain NixOS/Darwin module. Use the `_` prefix for the filename so import-tree excludes it (mkHost imports scopes explicitly):

```nix
# modules/scopes/_my-feature.nix
{config, lib, pkgs, ...}: let
  hS = config.hostSpec;
in {
  config = lib.mkIf hS.useMyFeature {
    environment.systemPackages = with pkgs; [ ... ];
    # services, config, etc.
  };
}
```

The scope is a plain NixOS module. `mkHost` imports it directly.

## 3. Add Home Manager Config (if needed)

For user-level configuration, add a plain HM module:

```nix
# modules/scopes/_my-feature-hm.nix
{config, lib, ...}: let
  hS = config.hostSpec;
in {
  config = lib.mkIf hS.useMyFeature {
    programs.something.enable = true;
  };
}
```

## 4. Add Persist Paths (if needed)

If the feature stores state, add persist paths in the same module:

```nix
home.persistence."/persist" = lib.optionalAttrs (!hS.isDarwin) {
  directories = [ ".local/share/my-feature" ];
};
```

## 5. Import in mkHost

Add the new scope to the module list in `mk-host.nix` so all hosts receive it (the `lib.mkIf` gate handles activation):

```nix
modules = [
  ./scopes/_my-feature.nix
  # ...
];
```

## 6. Add Tests

- **Eval test** in `modules/tests/eval.nix` -- verify the scope activates/deactivates
- **VM test** in `modules/tests/vm.nix` -- verify runtime behavior (if applicable)

## 7. Update Docs

Update documentation (flags table, module tree, scopes table).

## Further Reading

- [The Scope System](../concepts/scopes.md) -- conceptual overview
- [Technical Scope Details](../../scopes/README.md) -- all scope modules
