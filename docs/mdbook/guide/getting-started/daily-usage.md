# Day-to-Day Usage

Your daily workflow after installation.

## The Core Loop

1. Edit Nix files in your fleet repo
2. `git add .` (Nix only sees tracked files)
3. Rebuild:
   - NixOS: `sudo nixos-rebuild switch --flake .#<hostname>`
   - macOS: `darwin-rebuild switch --flake .#<hostname>`
4. Done. Changes are live.

## Common Commands

| Command | What it does |
|---------|-------------|
| `sudo nixos-rebuild switch --flake .#<hostname>` | Rebuild NixOS and switch |
| `darwin-rebuild switch --flake .#<hostname>` | Rebuild macOS and switch |
| `nix run .#validate` | Run all checks (format, eval, builds) |
| `nix run .#validate -- --vm` | Include slow VM integration tests |
| `nix fmt` | Format all Nix files |

## Updating Inputs

```sh
# Update everything
nix flake update

# Update just one input
nix flake update nixfleet
```

After updating, rebuild with the appropriate command for your platform.

## Rolling Back

On NixOS, select a previous generation from the boot menu, or:

```sh
sudo nixos-rebuild switch --rollback
```

On macOS:

```sh
darwin-rebuild switch --rollback
```

## Development Workflow

Use `nix develop` to enter the devShell with git hooks activated:

- **pre-commit:** Format check (`nix fmt --fail-on-change`)
- **pre-push:** Full validation (`nix run .#validate`)

## Next Steps

- [The Scope System](../concepts/scopes.md) -- understand how features are organized
- [Testing](../development/testing.md) -- the test pyramid
- [Adding a New Host](../advanced/new-host.md) -- expand your fleet
