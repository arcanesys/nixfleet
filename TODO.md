# TODO

Future work that cannot be closed inside this repository.

## External dependencies

- [ ] **#22: Revert `attic` input to upstream** when
  https://github.com/zhaofengli/attic/pull/300 is merged. External
  dependency — cannot be fixed in this repo until upstream lands.

- [ ] **Cosmetic:** Generation count fix in compliance probes (count
  the active system as generation 1, not 0). Lives in the
  `nixfleet-compliance` repository, not this one.

## Internal

- [x] **CLI: persistent deploy logs.** Write a full log of every
  deploy/release operation to `~/.local/state/nixfleet/logs/` regardless
  of verbosity. Should capture all subprocess invocations (command,
  stdout, stderr, exit code), tracing events, timestamps, and host
  context. Decisions needed: one file per operation vs rotating log,
  retention policy, format (plain text vs structured JSON).

- [ ] **Darwin fleet participation.** macOS hosts can build configs
  (`mkHost` → `darwinSystem`) but cannot participate in fleet
  orchestration. The agent's health module (`agent/src/health/systemd.rs`)
  is hardcoded to systemd, service modules (`_agent.nix`,
  `_control-plane.nix`) use `systemd.services` only, and there are no
  Darwin eval tests or test hosts in `fleet.nix`. Minimum viable:
  launchd agent service module, platform-abstracted health checks in
  Rust (`HealthChecker` trait with `SystemdChecker`/`LaunchdChecker`),
  and at least one Darwin host in eval tests.

- [x] **Cross-platform deploy: `--eval-only` for `release create`.**
  An operator on macOS cannot `nix build` Linux closures (and vice
  versa) without remote builders. The release/rollout path already
  pushes to a cache and agents pull — the local build step is not
  strictly necessary. Add `--eval-only` to `release create`: evaluate
  `config.system.build.toplevel.outPath` (instant, platform-agnostic),
  skip `nix build`, assume closures are in the cache (CI-built), and
  register the release with the CP as normal. Document remote builders
  as the recommended setup for mixed-platform fleets.

- [x] **Tests: serialize wiremock integration tests.** Shared `Mutex`
  in `cli/tests/scenarios/harness.rs`. `cli_lock()` serializes all
  tests that spawn the real binary via `assert_cmd`; `env_lock()`
  serializes tests that mutate `NIXFLEET_*` / `HOSTNAME` env vars.
