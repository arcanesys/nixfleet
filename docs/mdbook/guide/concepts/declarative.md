# Declarative Configuration

How this config turns flags into fully configured systems.

## The Idea

Instead of running commands to install and configure software, you declare the desired state:

```nix
# "I want a NixOS machine with dev tools and impermanent root"
hostSpec = {
  isDev = true;
  isImpermanent = true;
};
```

Nix figures out the rest: packages, services, config files.

## How It Works

The config uses a **conditional module pattern**:

1. **Host definitions** declare flags via `hostSpec` (what the machine should be)
2. **Scope modules** self-activate based on those flags (`lib.mkIf hS.isDev ...`)
3. **Core modules** provide the universal base (networking, users, security)
4. **Home Manager** configures user-level tools (shell, editor, git)

Scopes are plain NixOS/Darwin modules imported by `mkHost`. No host ever lists features manually. Add a new scope module, and every host with the matching flag gets it automatically.

## Smart Defaults

Fleet repos can define smart defaults where flags propagate intelligently. For example, enabling a compositor flag could automatically enable `isGraphical`. These are `mkDefault` values -- overridable per-host if needed.

## The Build

When you run `sudo nixos-rebuild switch --flake .#hostname`:

1. Nix evaluates all modules for your host
2. Derivations are built (or fetched from cache)
3. The new system generation is activated atomically
4. If anything fails, the previous generation remains active

## Further Reading

- [The Scope System](scopes.md) — how features are organized
- [Technical Module Details](../../core/README.md) — core module internals
