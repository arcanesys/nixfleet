# Dev Tools

Fleet repos can define a dev scope that activates with a `hostSpec` flag (e.g., `isDev`). The framework does not ship a dev scope -- it is fleet-specific.

## Typical Dev Scope

A consuming fleet might provide:

- **Build tools** -- gcc, make, cmake, pkg-config
- **Language managers** -- mise, asdf, or Nix-managed runtimes
- **direnv** -- automatic environment loading per project
- **Docker** -- containerization (NixOS only)

## Per-Project Environments

With direnv and Nix flakes, each project gets its own isolated environment:

```sh
cd my-project    # direnv auto-loads the devShell
```

No global package pollution. No version conflicts.

## Defining a Dev Scope

In your fleet repo, create a scope module gated by your dev flag:

```nix
{ config, lib, pkgs, ... }: let
  hS = config.hostSpec;
in {
  config = lib.mkIf hS.isDev {
    virtualisation.docker.enable = true;
    environment.systemPackages = with pkgs; [ gcc gnumake cmake ];
  };
}
```

Set `isDev = true` in a host's `hostSpec` to activate it.

## Further Reading

- [The Scope System](../concepts/scopes.md) -- how scopes work
- [Adding a New Scope](../advanced/new-scope.md) -- step-by-step guide
