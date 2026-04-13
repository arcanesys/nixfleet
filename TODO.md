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

- [x] **CLI: consolidate sync/async subprocess functions.** `release.rs`
  converted to async, `deploy.rs` imports from `crate::release::`.

- [x] **Agent: survive self-switch without restart rate-limiting.**
  When the agent applies a generation that changes its own systemd
  service, `switch-to-configuration` stops and restarts the agent.
  If the agent previously hit the activation lock (concurrent
  `nh os switch` / `nixos-rebuild`), the failed attempts count toward
  systemd's `StartLimitBurst` and the post-switch restart gets
  rate-limited — leaving the agent dead for minutes. Observed:
  agent killed at 14:31:38, not restarted until manual intervention
  13 minutes later despite `Restart=always` + `RestartSec=30`.
  Potential fixes: increase `StartLimitIntervalSec`/`StartLimitBurst`
  in the service module, add `StartLimitAction=none`, or use a
  watchdog timer that detects the agent is down and force-starts it.
  Also consider: the agent should detect the activation lock and
  back off instead of failing (reducing restart count toward the limit).

- [x] **Agent liveness in `nixfleet status`.** The `LAST SEEN` column
  shows the timestamp of the last agent report, but there's no visual
  indicator when a machine hasn't reported in a suspiciously long time.
  A machine could be dead for hours and the operator would only notice
  by reading timestamps. Add a staleness threshold (e.g. 2× poll
  interval) — machines that haven't reported within the threshold
  should show a warning state (e.g. `stale` or `unreachable`) in the
  STATUS column. Consider also adding `--watch` to `nixfleet status`
  for live polling.
