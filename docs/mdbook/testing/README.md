# Testing

## Purpose

Tests follow a 3-tier pyramid ensuring config correctness at build time and runtime behavior in VMs.

## Location

- `modules/tests/eval.nix` -- Tier C eval tests
- `modules/tests/vm.nix` -- Tier A VM tests
- `modules/tests/_lib/helpers.nix` -- Test helper library

## Test Pyramid

| Tier | Type | Speed | Location | Trigger |
|------|------|-------|----------|---------|
| **C** | [Eval](eval-tests.md) | Instant | `eval.nix` | `nix flake check`, `nix run .#validate` |
| **A** | [VM](vm-tests.md) | Slow | `vm.nix` | `nix run .#validate -- --vm` |
| **B** | Smoke | -- | `smoke.sh` (future) | Post-deploy verification |

## Running Tests

```sh
# Eval tests only (instant)
nix flake check --no-build

# Full validation (format + eval + builds)
nix run .#validate

# Include VM tests (slow, x86_64-linux only)
nix run .#validate -- --vm
```

## Helper Library

`modules/tests/_lib/helpers.nix` provides `mkEvalCheck` which creates a `pkgs.runCommand` that fails if any assertion in a list is false. Each assertion has a `check` (boolean) and `msg` (description).

## Adding Tests

1. New scope/feature -> add eval assertions in `eval.nix`
2. Runtime behavior -> add VM test in `vm.nix`
3. VM tests use `mkTestNode` helper (stubs agenix, provides test passwords)

## Links

- [validate app](../apps/validate.md)
- [Architecture](../architecture.md)
