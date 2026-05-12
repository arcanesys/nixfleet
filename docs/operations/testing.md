# Testing your fleet config

NixFleet ships a `validate` test runner that gates every level of verification - format, flake check, host eval, system builds, Rust unit + integration tests, and VM-harness scenarios. Run it before every push to the fleet repo.

```sh
nix run .#validate              # fast: format + flake check + eval + host builds
nix run .#validate -- --rust    # + cargo nextest + clippy + nix-sandbox builds
nix run .#validate -- --vm      # + every fleet-harness-* scenario
nix run .#validate -- --all     # everything
```

## Modes

| Mode | What it runs | Typical time |
|---|---|---|
| (default) | `nix fmt --check`, `nix flake check`, every `nixosConfigurations.*` host eval, every host's `system.build.toplevel` build | seconds to a few minutes |
| `--rust` | adds `cargo nextest run`, `cargo clippy --all-targets --all-features`, all `crates/*/tests/` integration tests, `nix-sandbox` builds | several minutes |
| `--vm` | adds every `fleet-harness-*` scenario (smoke, signed-roundtrip, auditor-chain, deadline-expiry, stale-target, boot-recovery, rollback-policy, concurrent-checkin, enroll-replay, ...) | tens of minutes |
| `--all` | everything above | longest |

## When to use which mode

- **Before every `git push`**: default mode. It's what CI runs anyway; running it locally first surfaces issues before they hit a runner.
- **Before opening a PR that touches Rust crates**: `--rust`. Clippy + nextest catch the regressions humans never spot in review.
- **When reproducing a fleet-harness regression**: `--vm`. The harness scenarios are deterministic; running locally lets you `journalctl` the VM during the failing flow.
- **Before tagging a release**: `--all`. Cold-cache cost; once per release is reasonable.

## Scenario catalogue

For the list of `fleet-harness-*` scenarios and what each one exercises, see [reference/harness](../reference/harness.md). The scenarios are intentionally narrow - each one isolates a single property of the signed-GitOps loop, the rollback machinery, or the reconciler.

## Interaction with CI

CI runs the default mode on every push and `--rust` + `--vm` on every release tag. Local `validate` and CI `validate` execute the same code path, so a green local run is a strong predictor of a green CI run. Drift between local and CI results is treated as a bug in the runner, not in your fleet config.
