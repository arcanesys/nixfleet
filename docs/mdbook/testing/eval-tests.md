# Eval Tests

Eval tests (Tier C in the [testing overview](overview.md)) assert configuration
properties at Nix evaluation time. They run instantly and catch structural
mistakes before anything is built.

> For the full test tier map (eval / integration / VM / Rust) see the
> [Testing Overview](overview.md). This page documents only the eval checks.

## How to run

```sh
nix flake check --no-build
```

The `--no-build` flag skips VM tests so only eval checks execute. Every check is
a `pkgs.runCommand` that prints `PASS:` or `FAIL:` for each assertion and exits
non-zero on the first failure.

## Test fleet

Eval tests run against a minimal test fleet defined in `modules/fleet.nix`. These
hosts exist solely to exercise framework config paths - they are not a real org.

The test fleet is defined in `modules/fleet.nix`. Key hosts used by eval checks:

| Host | Key config | Purpose |
|------|------------|---------|
| `web-01` | workstation role, impermanence enabled | Default web server, impermanent root |
| `web-02` | workstation role, impermanence enabled | SSH hardening tests |
| `dev-01` | `userName = "alice"` | Custom user override |
| `edge-01` | endpoint role | Minimal edge device |
| `srv-01` | server role | Production server |
| `agent-test` | agent enabled, tags, health checks | Agent module options |

Additional hosts (`secrets-test`, `infra-test`, `cache-test`, `microvm-test`, `backup-restic-test`) exercise other subsystems. All hosts share org-level defaults and use `isVm = true`.

## Current checks

| Check | Host | What it asserts |
|-------|------|-----------------|
| `eval-ssh-hardening` | web-02 | `PermitRootLogin == "prohibit-password"`, `PasswordAuthentication == false`, firewall enabled |
| `eval-hostspec-defaults` | web-01 | `userName` is non-empty, `hostName` matches `"web-01"` |
| `eval-username-override` | web-01, dev-01 | web-01 uses the shared default user; dev-01 overrides it to a different value |
| `eval-locale-timezone` | web-01 | `timeZone`, `defaultLocale`, `console.keyMap` are all non-empty |
| `eval-ssh-authorized` | web-01 | Primary user and root both have at least one SSH authorized key |
| `eval-password-files` | web-01 | `hostSpec` exposes `hashedPasswordFile` and `rootHashedPasswordFile` options |
| `eval-agent-tags-health` | agent-test | Agent systemd service has `NIXFLEET_TAGS = "web,production"`, health-checks.json config file exists |

## Adding a new eval test

1. Pick (or add) a test fleet host in `modules/fleet.nix` that exercises the
   config path you want to verify.

2. Add a new check in `modules/tests/eval.nix` following this pattern:

```nix
eval-my-check = let
  cfg = nixosCfg "web-01";
in
  mkEvalCheck "my-check" [
    {
      check = cfg.some.option == expectedValue;
      msg = "web-01 some.option should be expectedValue";
    }
  ];
```

3. Run `nix flake check --no-build` to verify the new assertion passes.

The `mkEvalCheck` helper (from `modules/tests/_lib/helpers.nix`) takes a check
name and a list of `{ check : bool; msg : string; }` assertions. It produces a
`runCommand` derivation that prints each result and fails on the first `false`.
