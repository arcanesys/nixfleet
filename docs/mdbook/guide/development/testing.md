# Testing Your Config

A 3-tier test pyramid ensures your config works before deploying to hardware.

## The Pyramid

| Tier | Speed | What it tests | Command |
|------|-------|--------------|---------|
| Eval | Instant | Config correctness (flags, options) | `nix flake check` |
| VM | Minutes | Runtime behavior (services, binaries) | `nix run .#validate -- --vm` |
| Smoke | Manual | Real-world state on live hardware | Post build-switch |

## Eval Tests (Tier C)

6 eval checks run instantly and catch configuration errors:
- SSH hardening options are set (PermitRootLogin, PasswordAuthentication, firewall)
- hostSpec defaults propagate correctly (userName, hostName)
- userName override works (shared defaults vs per-host override)
- Locale, timezone, and keyboard settings are set
- SSH authorized keys propagate to user and root
- Password file options exist on hostSpec

Run them with:
```sh
nix flake check --no-build    # eval only, no builds
nix run .#validate            # includes eval + builds
```

## VM Tests (Tier A)

These boot real NixOS VMs and verify runtime behavior:
- **vm-core** -- SSH, NetworkManager, firewall, user/groups, zsh, git
- **vm-minimal** -- negative test: minimal host with core services only

Run them with:
```sh
nix run .#validate -- --vm    # x86_64-linux only
```

## Pre-commit and Pre-push

Git hooks enforce quality:
- **pre-commit** -- format check
- **pre-push** -- full validation

## When to Add Tests

- New hostSpec flag? Add eval assertions
- New scope with services? Add VM test cases
- New runtime behavior? Add to the appropriate VM suite

## Further Reading

- [VM Testing](vm-testing.md) -- detailed VM test guide
- [Technical Test Details](../../testing/README.md) -- test implementation
