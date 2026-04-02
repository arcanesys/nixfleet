# Eval Tests (Tier C)

## Purpose

Assert config properties at evaluation time. No builds, no VMs — instant feedback. Each check evaluates NixOS config attributes and fails if assertions are false.

## Location

- `modules/tests/eval.nix`
- `modules/tests/_lib/helpers.nix`

## Test Suites (6 checks)

### eval-ssh-hardening
Verifies SSH security config on `web-02`:
- `PermitRootLogin = "prohibit-password"`
- `PasswordAuthentication = false`
- Firewall enabled

### eval-hostspec-defaults
Verifies framework hostSpec defaults propagate on `web-01`:
- `userName` is set (non-empty)
- `hostName` matches `"web-01"`

### eval-username-override
Verifies `userName` inheritance and override:
- `web-01` has `userName` from shared defaults
- `dev-01` overrides to a different `userName` than the shared default

### eval-locale-timezone
Verifies locale/timezone/keyboard settings on `web-01`:
- `time.timeZone` is non-empty
- `i18n.defaultLocale` is non-empty
- `console.keyMap` is non-empty

### eval-ssh-authorized
Verifies SSH authorized keys from shared defaults on `web-01`:
- Primary user has at least one authorized key
- Root has at least one authorized key

### eval-password-files
Verifies password file options exist on hostSpec:
- `hostSpec.hashedPasswordFile` option present
- `hostSpec.rootHashedPasswordFile` option present

## Platform

x86_64-linux only (all test hosts are x86_64-linux configs).

## Running

```sh
nix flake check --no-build         # all eval checks, instant
nix build .#checks.x86_64-linux.eval-ssh-hardening --no-link  # one check
```

## Links

- [Testing Overview](README.md)
- [VM Tests](vm-tests.md)
