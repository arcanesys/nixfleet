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

- [ ] **Darwin fleet participation.** macOS hosts can build configs
  (`mkHost` → `darwinSystem`) but cannot participate in fleet
  orchestration. The agent's health module (`agent/src/health/systemd.rs`)
  is hardcoded to systemd, service modules (`_agent.nix`,
  `_control-plane.nix`) use `systemd.services` only, and there are no
  Darwin eval tests or test hosts in `fleet.nix`. Minimum viable:
  launchd agent service module, platform-abstracted health checks in
  Rust (`HealthChecker` trait with `SystemdChecker`/`LaunchdChecker`),
  and at least one Darwin host in eval tests.
