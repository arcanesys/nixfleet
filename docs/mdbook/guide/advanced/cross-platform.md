# Cross-Platform Design

How one config targets NixOS, macOS, and portable environments.

## The Challenge

NixFleet supports:
- **NixOS** -- full system control
- **macOS** -- nix-darwin with Home Manager

Each platform has different capabilities. The framework handles this with guards and platform-aware modules.

## Platform Guards

```nix
# Darwin-only code
lib.mkIf hS.isDarwin { ... }

# NixOS impermanence paths
lib.mkIf hS.isImpermanent { ... }

# Home persistence (option doesn't exist on Darwin)
lib.optionalAttrs (!hS.isDarwin) {
  home.persistence."/persist" = { ... };
}
```

## Three Layers

| Layer | NixOS | macOS | Portable |
|-------|-------|-------|----------|
| System config | NixOS modules | nix-darwin modules | N/A |
| User config | Home Manager | Home Manager | Wrapper configs |
| Config files | `_config/` | `_config/` | `_config/` |

The `_config/` directory is the shared source of truth. Both Home Manager and wrappers read from it, ensuring the same experience everywhere.

## What Is Platform-Specific

- **NixOS only:** impermanence, systemd services, Wayland compositors, Docker
- **macOS only:** Homebrew, Karabiner, AeroSpace, Dock preferences
- **Both:** shell, editor, git, SSH, dev tools (via Home Manager)
- **Portable:** shell + terminal wrappers (subset of the full config)

## Design Principle

When a cross-platform approach adds too much complexity, make it platform-specific and keep it simple. Note ambitious ideas as TODOs rather than implementing fragile workarounds.

## Further Reading

- [Portable Environments](../concepts/portable.md) — wrappers in detail
- [Technical Architecture](../../architecture.md) — module structure
