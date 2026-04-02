# validate

## Purpose

Full validation suite: formatting check, eval tests, host builds, and optional VM integration tests.

## Location

- `modules/apps.nix` (the `validate` app definition)

## Usage

```sh
nix run .#validate              # default: format + eval + builds
nix run .#validate -- --vm      # include VM integration tests
nix run .#validate -- --fast    # (reserved for future use)
```

## Validation Steps

1. **Formatting** -- `nix fmt --fail-on-change`
2. **Eval tests** (Linux only) -- 6 checks covering SSH hardening, hostSpec defaults, username override, locale/timezone, SSH authorized keys, password file options
3. **NixOS host builds** -- all hosts in `nixosConfigurations` (web-01, web-02, edge-01, dev-01, srv-01)
4. **VM integration tests** (with `--vm`) -- vm-core, vm-minimal

## Output

Color-coded pass/fail/skip for each check, with summary counts.

## Dependencies

- Pre-push hook runs this automatically
- VM tests require x86_64-linux

## Links

- [Apps Overview](README.md)
- [Eval Tests](../testing/eval-tests.md)
- [VM Tests](../testing/vm-tests.md)
