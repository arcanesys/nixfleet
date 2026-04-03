# hostSpec Options

All options declared in the framework's `hostSpec` module. Fleet repos can extend hostSpec with additional options via plain NixOS modules.

## Data fields

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `hostName` | `str` | -- (required) | The hostname of the host. Set automatically by mkHost. |
| `userName` | `str` | -- (required) | The username of the primary user. |
| `home` | `str` | `/home/<userName>` (Linux) or `/Users/<userName>` (Darwin) | Home directory path. Computed from `userName` and `isDarwin`. |
| `timeZone` | `str` | `"UTC"` | IANA timezone (e.g., `Europe/Paris`). |
| `locale` | `str` | `"en_US.UTF-8"` | System locale. |
| `keyboardLayout` | `str` | `"us"` | XKB keyboard layout. |
| `networking` | `attrsOf anything` | `{}` | Attribute set of networking information (e.g., `{ interface = "enp3s0"; }`). |
| `sshAuthorizedKeys` | `listOf str` | `[]` | SSH public keys added to `authorized_keys` for both the primary user and root. |
| `secretsPath` | `nullOr str` | `null` | Hint for secrets repo path. Framework-agnostic -- no tool coupling. |
| `hashedPasswordFile` | `nullOr str` | `null` | Path to hashed password file for the primary user. When non-null, sets `users.users.<userName>.hashedPasswordFile`. |
| `rootHashedPasswordFile` | `nullOr str` | `null` | Path to hashed password file for root. When non-null, sets `users.users.root.hashedPasswordFile`. |

## Capability flags

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `isMinimal` | `bool` | `false` | Minimal host. When true, the base scope skips CLI packages. |
| `isDarwin` | `bool` | `false` | Darwin (macOS) host. Set automatically by mkHost for Darwin platforms. |
| `isImpermanent` | `bool` | `false` | Impermanent host. Enables btrfs root wipe and persistence paths. |
| `isServer` | `bool` | `false` | Server host. When true, the primary user is not added to `trusted-users`. |

## Extending hostSpec

Fleet repos add custom flags via plain NixOS modules:

```nix
{lib, ...}: {
  options.hostSpec = {
    isDev = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Enable development tools.";
    };
    isGraphical = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Enable graphical environment.";
    };
  };
}
```

Include the extension module in your mkHost `modules` list. Framework-level hostSpec options and fleet-level extensions merge naturally through the NixOS module system.
