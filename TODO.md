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

- [ ] **CLI: persistent deploy logs.** Write a full log of every
  deploy/release operation to `~/.local/state/nixfleet/logs/` regardless
  of verbosity. Should capture all subprocess invocations (command,
  stdout, stderr, exit code), tracing events, timestamps, and host
  context. Decisions needed: one file per operation vs rotating log,
  retention policy, format (plain text vs structured JSON).

- [ ] **Tests: serialize wiremock integration tests.** `cargo test`
  runs scenario tests in parallel, causing flaky failures. The real
  `nixfleet` binary spawned by `assert_cmd` leaks env vars / config
  state between concurrent tests. `cargo nextest` (process isolation)
  is unaffected. Simplest fix: set `--test-threads=1` for the scenario
  binary via `.cargo/config.toml`, or add a shared `Mutex` in the test
  harness to serialize tests that spawn the binary.
