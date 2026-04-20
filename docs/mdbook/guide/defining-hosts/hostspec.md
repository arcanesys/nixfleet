# hostSpec Configuration

`hostSpec` is a NixOS module option that holds host identity data. It is the primary mechanism for identifying hosts in NixFleet.

Every module injected by mkHost - core, scopes, Home Manager - can read `config.hostSpec` to adapt behavior. Scope activation is driven by `nixfleet.<scope>.enable` options (set by roles from nixfleet-scopes), not by hostSpec flags.

## Options

**Data fields:** `userName` (required), `hostName` (auto-set), `home` (computed), `timeZone`, `locale`, `keyboardLayout`, `sshAuthorizedKeys`, `networking`, `secretsPath`, `hashedPasswordFile`, `rootHashedPasswordFile`.

**Platform flag:** `isDarwin` (auto-set by mkHost).

For the full option reference with types, defaults, and descriptions, see [hostSpec Options](../../reference/hostspec-options.md).

## Accessing hostSpec in modules

hostSpec is available in any NixOS, Darwin, or Home Manager module injected by mkHost:

```nix
# In a NixOS/Darwin module
{config, lib, ...}: let
  hS = config.hostSpec;
in {
  services.myapp.dataDir = "${hS.home}/data";
  networking.firewall.enable = lib.mkIf config.nixfleet.firewall.enable true;
}
```

```nix
# In a Home Manager module
{config, lib, ...}: let
  hS = config.hostSpec;
in {
  programs.git.userName = lib.mkIf (!hS.isDarwin) "linux-user";
}
```

Home Manager modules receive hostSpec because mkHost imports the hostSpec module into the HM evaluation and passes the effective hostSpec values.

## Extending hostSpec in fleet repos

The framework defines only the options above. Fleet repos add their own flags as plain NixOS modules:

```nix
# modules/hostspec-extensions.nix (in your fleet repo)
{lib, ...}: {
  options.hostSpec = {
    isDev = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Enable development tools and Docker.";
    };
    isGraphical = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Enable graphical desktop (audio, fonts, display manager).";
    };
  };
}
```

Then use them in fleet-level scopes:

```nix
# modules/scopes/dev.nix (in your fleet repo)
{config, lib, pkgs, ...}: let
  hS = config.hostSpec;
in {
  config = lib.mkIf hS.isDev {
    virtualisation.docker.enable = true;
    environment.systemPackages = with pkgs; [gcc gnumake];
  };
}
```

Include the extension module in your `mkHost` calls via the `modules` parameter. No framework changes needed.

## Org defaults pattern

Define shared defaults in a `let` binding and merge per-host:

```nix
let
  orgDefaults = {
    userName = "deploy";
    timeZone = "America/New_York";
    locale = "en_US.UTF-8";
    keyboardLayout = "us";
    sshAuthorizedKeys = [
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA... ops-team"
    ];
  };
in {
  web-01 = mkHost {
    hostName = "web-01";
    platform = "x86_64-linux";
    hostSpec = orgDefaults;
    modules = [nixfleet-scopes.scopes.roles.server ./hosts/web-01/hardware.nix];
  };
}
```

All hostSpec values passed to mkHost use `lib.mkDefault`, so modules in the `modules` list can override them if needed.
