# hostSpec Configuration

`hostSpec` is a NixOS module option that holds host identity data and capability flags. It is the primary mechanism for differentiating hosts in NixFleet.

Every module injected by mkHost -- core, scopes, Home Manager -- can read `config.hostSpec` to adapt behavior. Scopes use hostSpec flags to self-activate via `lib.mkIf`.

## Data fields

These carry host identity and environment information.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `userName` | `str` | *required* | Primary user account name. |
| `hostName` | `str` | *required* | Machine hostname. Set by mkHost, always matches the `hostName` parameter. |
| `home` | `str` | computed | Home directory. `/home/<userName>` on Linux, `/Users/<userName>` on Darwin. |
| `timeZone` | `str` | `"UTC"` | IANA timezone (e.g. `Europe/Paris`). |
| `locale` | `str` | `"en_US.UTF-8"` | System locale. |
| `keyboardLayout` | `str` | `"us"` | XKB keyboard layout. |
| `sshAuthorizedKeys` | `listOf str` | `[]` | SSH public keys added to both the primary user and root `authorized_keys`. |
| `secretsPath` | `nullOr str` | `null` | Hint for the secrets repo path. Framework-agnostic -- no coupling to agenix or sops. |
| `networking` | `attrsOf anything` | `{}` | Freeform networking data (e.g. `{ interface = "eno1"; }`). |
| `hashedPasswordFile` | `nullOr str` | `null` | Path to hashed password file for the primary user. |
| `rootHashedPasswordFile` | `nullOr str` | `null` | Path to hashed password file for root. |

## Capability flags

These control which scopes and behaviors activate.

| Flag | Type | Default | Controls |
|------|------|---------|----------|
| `isMinimal` | `bool` | `false` | When `true`, the base scope skips CLI packages. Use for lean servers and edge devices. |
| `isDarwin` | `bool` | `false` | Auto-set by mkHost based on `platform`. Do not set manually. Switches core module, home directory path, and HM modules. |
| `isImpermanent` | `bool` | `false` | Activates the impermanence scope: btrfs root wipe, system/user persist paths. |
| `isServer` | `bool` | `false` | Marks a server host. Used by core module to restrict `trusted-users`. |

## Accessing hostSpec in modules

hostSpec is available in any NixOS, Darwin, or Home Manager module injected by mkHost:

```nix
# In a NixOS/Darwin module
{config, lib, ...}: let
  hS = config.hostSpec;
in {
  services.myapp.dataDir = "${hS.home}/data";
  networking.firewall.enable = lib.mkIf hS.isServer true;
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
    hostSpec = orgDefaults // {
      isServer = true;
      isMinimal = true;
    };
  };
}
```

All hostSpec values passed to mkHost use `lib.mkDefault`, so modules in the `modules` list can override them if needed.
