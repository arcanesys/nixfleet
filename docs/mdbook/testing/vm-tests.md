# VM Tests

VM tests boot real NixOS virtual machines under QEMU and assert runtime state
via Python test scripts run by the nixosTest driver. They verify services
start, ports listen, multi-node interactions work end-to-end, and rollout
state machines behave as documented.

## How to run

All VM checks are discovered dynamically by the `validate` script:

```sh
nix run .#validate -- --vm          # format + eval + hosts + all vm-* checks
nix run .#validate -- --all         # same + cargo test --workspace
```

Individual tests (useful for iterating on one scenario):

```sh
nix build .#checks.x86_64-linux.vm-fleet --no-link
nix build .#checks.x86_64-linux.vm-fleet-release --no-link
nix build .#checks.x86_64-linux.vm-fleet-apply-failure --no-link
# … and so on, any attribute under .#checks.<system> starting with `vm-`
```

`nix log /nix/store/<hash>-vm-test-run-<name>.drv` retrieves the full driver
output for a failed or past run.

## Requirements

- **Platform:** x86\_64-linux only (nixosTest uses QEMU)
- **KVM:** `/dev/kvm` for acceptable performance
- **Disk space:** each VM test builds a NixOS closure; expect several GB per test
- **Time:** minutes per test (closure build + parallel VM boots + assertions)

## Test cycle

Each VM test goes through:

1. **Build** — Nix evaluates the nodes' config and builds each node's system closure.
2. **Boot** — QEMU launches one or more VMs in parallel; the shared host
   `/nix/store` is mounted read-only over 9p on every VM.
3. **Assert** — a Python test script runs commands via the test driver API
   (`machine.succeed()`, `machine.fail()`, `machine.wait_for_unit()`,
   `machine.wait_until_succeeds(cmd, timeout=N)`).
4. **Cleanup** — VMs shut down, driver reports pass/fail.

## Framework-level VM tests

These test one subsystem in isolation. Most are defined in `modules/tests/vm*.nix`.

### `vm-core`

Boots a standard framework node (`defaultTestSpec`, no special flags) and verifies:

- `multi-user.target` reached
- `sshd` and `NetworkManager` running
- Firewall active (nftables input chain exists)
- Test user exists in the `wheel` group
- Core packages available to the user (`zsh`, `git`)

This is the "does everything still boot" smoke test.

### `vm-minimal`

Boots a node with `isMinimal = true` and verifies the minimal profile stays
minimal:

- `multi-user.target` reached
- Core tools still present (`zsh`, `git` come from `core/nixos.nix`, not
  the base scope)
- Graphical/dev tools absent (e.g., `niri` not installed, Docker not running)

### `vm-infra`

One node, four scopes in one VM for speed:

- **Firewall** — nftables active, SSH rate limiting rules present
  (`limit rate 5/minute`), drop logging enabled.
- **Monitoring** — node exporter running, port 9100 responds with
  Prometheus text, `node_systemd` collector active.
- **Backup** — systemd timer registered, manual trigger writes
  `status.json` with `"status": "success"`.
- **Secrets** — SSH host key generated at
  `/etc/ssh/ssh_host_ed25519_key` with mode 600.

### `vm-nixfleet`

Minimal CP ↔ agent handshake (2 nodes):

1. CP starts, `nixfleet-control-plane.service` listens on 8080.
2. Agent starts with `pollInterval = 2`, `dryRun = true`.
3. Test bootstraps an admin API key, creates a release + rollout.
4. Rollout executor sets the agent's desired generation.
5. Agent detects mismatch, runs dry-run cycle (skips apply), reports back.
6. CP inventory reflects the agent's report.

This is the lowest-level end-to-end proof that the agent and CP can
actually talk to each other.

### `vm-agent-rebuild`

The fetch → apply → verify pipeline with two sub-scenarios:

- **Test B (no-cache)**: closure pre-seeded in agent store, agent verifies
  path exists via `nix path-info` and reports up-to-date.
- **Test C (missing path guard)**: non-existent store path, no cache URL,
  agent detects the missing path and reports an error without advancing
  its generation.

### `vm-fleet` — "Tier A headline test"

4-node fleet: `cp` + `web-01` + `web-02` + `db-01`, with full mTLS
(build-time CA + CP server cert + per-agent client certs, no
`allowInsecure`).

1. CP bootstraps an admin API key.
2. All 3 agents register with tags (web × 2, db × 1).
3. **Canary rollout** on tag `web` (strategy `staged`, batch sizes `["1","100%"]`)
   — both agents healthy, rollout reaches `completed`.
4. **Health-gate failure rollout** on tag `db` (strategy `all_at_once`) — db-01's
   health check points at `http://localhost:9999/health` which nothing listens
   on; the rollout hits `health_timeout` and pauses.
5. **Resume** the paused rollout and verify it transitions out of `paused`.
6. **Metrics** — CP `/metrics` exposes `nixfleet_fleet_size` and
   `nixfleet_rollouts_total`; agent node exporter on web-01 exposes
   `node_cpu`.

## Phase 3 fleet scenario subtests

Every CLI path, failure mode, and rollout branch from the Phase 3 design
spec has its own independently buildable VM subtest under
`modules/tests/_vm-fleet-scenarios/*.nix`. The aggregator
`modules/tests/vm-fleet-scenarios.nix` exposes each one as
`.#checks.<system>.vm-fleet-<name>`.

### `vm-fleet-tag-sync` (M3)

Real agent with `tags = ["web" "canary" "eu-west"]` in NixOS config. Asserts
tags appear in the CP `machine_tags` table after the first health report,
that filtering by a declared tag returns the agent, and that undeclared tags
do not leak into the table.

### `vm-fleet-bootstrap` (D1)

End-to-end bootstrap flow:

1. Start CP with an empty `api_keys` table.
2. Operator runs `nixfleet bootstrap --name test-admin` — the CLI returns
   the first admin API key over mTLS.
3. Use the returned key to `list machines` (empty), wait for two real agents
   (`web-01`, `web-02`) to register, `list machines` again (2 visible).
4. Create a release via `POST /api/v1/releases` pointing at each agent's
   real `/run/current-system` toplevel.
5. POST a rollout targeting `tag=web` and wait for `status=completed`.
6. **Negative**: a second `nixfleet bootstrap` call must fail (409 Conflict).

### `vm-fleet-release` (R1, R2)

Real `nixfleet release create --push-to ssh://root@cache` exercised against
a harmonia binary cache server:

- Uses the shared `nix-shim` (`modules/tests/_lib/nix-shim.nix`) to intercept
  `nix eval` and `nix build` on the builder node — returns a canned store
  path — while delegating `nix copy` to the real nix so the binary transfer
  actually happens.
- Cache node runs `services.nixfleet-cache-server` (harmonia) with a
  build-time signing key baked as a `/nix/store` path (avoids the
  `CREDENTIALS=243` race documented in TODO.md).
- Post-push, assert via the VM-local Nix database (`nix-store -q --references`)
  that the path is registered on `cache` and NOT on `cp`.
- Agent then fetches from `http://cache:5000` and the DB check passes on
  the agent too.

### `vm-fleet-deploy-ssh` (D4)

Real `nixfleet deploy --hosts target --ssh --target root@target` — no CP
in the topology at all. The CLI calls `nix eval` (shim) → `nix build`
(shim) → `nix-copy-closure` (real) → `ssh target switch-to-configuration`
(real). A stub `switch-to-configuration` writes a marker file to `/tmp`
that the test asserts. Proves `--ssh` mode truly bypasses the CP.

### `vm-fleet-apply-failure` (F1, RB1)

Command health check with a sentinel file
(`/var/lib/fail-next-health`) drives the failure path:

1. Sentinel file created before the agent starts → first health report is
   unhealthy → rollout pauses (F1).
2. Assert `current_generation` is still the agent's original toplevel (RB1
   — the agent did not advance to the failing generation).
3. Clear the sentinel, wait for `health_reports.all_passed = 1`, call
   `POST /api/v1/rollouts/{id}/resume`, assert the rollout reaches
   `completed`.

This test covers two subtle bugs in the resume path: the rollout
executor must not re-mark a batch unhealthy from stale pre-resume
reports, and the agent's `CommandChecker` must use an absolute `/bin/sh`
so it works under a systemd unit PATH. A regression in either would
make this test hang at Phase 9 (resume → completed).

### `vm-fleet-revert` (F2, C3)

2-agent staged rollout with `on_failure = revert`:

- Both agents healthy → first batch `succeeded`.
- Test then arms the sentinel on both agents so the *next* batch fails.
- Rollout executor walks `previous_generations` on succeeded batches and
  restores the per-machine desired generation.
- Indirectly covers C3 (HealthRunner::run_all actually runs post-deploy) —
  if the health runner were dead code, the failing report would never
  arrive and the revert path wouldn't fire.

### `vm-fleet-timeout` (F3)

The agent is configured but its unit's `wantedBy` is forced to `[]` so the
process never starts. CP records the machine in the release but sees zero
reports from it. The batch sits in `pending_count > 0` until
`health_timeout` elapses, at which point `evaluate_batch` pushes
`pending_count` into `unhealthy_count` and marks the batch failed.

Negative control: the `reports` table is empty for the machine — the pause
reason really is "timeout", not "agent reported a failure".

### `vm-fleet-poll-retry` (F7)

Agent starts *before* the CP. First poll hits a closed port (connection
refused). The agent's main loop schedules a retry at `retryInterval = 5s`.
Then the CP starts, and the agent's next retry succeeds. Asserts the
agent journal contains the retry-scheduling log line, then waits for
registration.

### `vm-fleet-mtls-missing` (A3)

Pure transport-layer test. CP has `tls.clientCa` set. A client with the CA
cert (can verify server) but no client key pair sends curl against
`/health` and `/api/v1/machines/{id}/report`:

- Without `--cert` → handshake failure at the TLS layer (asserted by
  grepping the curl verbose output for any of a set of TLS markers:
  `alert`, `handshake`, `certificate required`, `SSL_ERROR`, etc.).
- Positive control with a valid client cert → HTTP response comes back
  (any status — what matters is the handshake completed).

### `vm-fleet-rollback-ssh` (RB2)

Real `nixfleet rollback --host target --ssh --generation <G1>` end-to-end:

1. Deploy stub `G2` via `nixfleet deploy --ssh` → target writes
   `active=g2` marker file.
2. Pre-copy `G1` to target via `nix-copy-closure` (rollback handler does
   NOT copy, it only SSHes and runs `<gen>/bin/switch-to-configuration`).
3. Run `nixfleet rollback --host target --ssh --generation <G1>` → target
   writes `active=g1` marker.
4. Assert both G1 and G2 are still registered in target's Nix DB (rollback
   did not delete the forward generation).

## Shared VM test helpers

All scenario tests use helpers from `modules/tests/_lib/helpers.nix`
(via `modules/tests/vm-fleet-scenarios.nix` which pre-binds them):

- **`mkCpNode { testCerts, ... }`** — a CP node with standard mTLS wiring
  (CA + server cert, `services.nixfleet-control-plane` with
  `clientCa`), `sqlite` and `python3` pre-installed.
- **`mkAgentNode { testCerts, hostName, tags, healthChecks, ... }`** — an
  agent node with standard TLS, fleet CA trust, `services.nixfleet-agent`
  with pre-wired `machineId`/`tags`/`dryRun`. Escape hatch
  `agentExtraConfig` (merged via `lib.recursiveUpdate` into
  `services.nixfleet-agent`) handles per-scenario overrides like
  `retryInterval` or `allowInsecure`.
- **`tlsCertsModule { testCerts, certPrefix }`** — a NixOS module fragment
  wiring the fleet CA plus a named client cert under `/etc/nixfleet-tls/`,
  for operator / builder / cache-style nodes that need TLS certs but
  aren't a CP or an agent.
- **`testPrelude { certPrefix ? "cp", api ? "https://localhost:8080" }`** —
  returns a Python prelude string with `TEST_KEY`, `KEY_HASH`, `AUTH`,
  `CURL`, `API` constants and a `seed_admin_key(node)` helper. Interpolate
  at the top of every `testScript`:

  ```nix
  testScript = ''
    ${testPrelude {}}
    cp.start()
    cp.wait_for_unit("nixfleet-control-plane.service")
    cp.wait_for_open_port(8080)
    seed_admin_key(cp)
    ...
  '';
  ```

- **`mkTlsCerts { hostnames }`** (from `_lib/tls-certs.nix`) — builds the
  fleet CA + per-host cert pairs at Nix-eval time. Deterministic, no
  runtime setup.
- **`nix-shim`** (from `_lib/nix-shim.nix`) — a `writeShellApplication`
  that intercepts `nix eval` / `nix build` with canned responses while
  delegating `nix copy` and other subcommands to the real nix at an
  immutable `${pkgs.nix}/bin/nix` path. The absolute path is
  deliberate: installing the shim into `systemPackages` would collide
  with the real nix at `/run/current-system/sw/bin/nix`, and if the
  shim won the collision its fall-through branches would infinitely
  exec themselves. See the nixosTest gotchas section below.

## nixosTest gotchas worth knowing

A few behaviours of the nixosTest framework itself that bit scenarios
during Phase 3:

- **Shared `/nix/store` via 9p**: every VM sees the host store read-only
  via 9p mount. Any store path referenced anywhere in the test evaluation
  is visible as a file on every node regardless of whether it was ever
  copied there. `test -e <storepath>` assertions are therefore invariant.
  The workaround is to check the VM-local Nix database
  (`nix-store -q --references <path>`) which *is* per-VM.
- **systemd PATH for services**: services like `nixfleet-agent` do not get
  `/run/current-system/sw/bin` in their `PATH` by default, so
  `Command::new("sh")` (relative lookup) fails with ENOENT. Use absolute
  paths like `/bin/sh`.
- **`nix` shim collisions**: adding a shim package named `"nix"` to
  `environment.systemPackages` causes a silent collision with the real
  `nix` in `/run/current-system/sw/bin/nix`. The workaround is to keep
  the shim *only* on `sessionVariables.PATH` (which still pulls it into
  the closure via string interpolation) and never in `systemPackages`.
- **`wait_for_unit` vs `wait_until_succeeds("systemctl is-active")`**:
  a systemd unit stuck in the `activating` state forever (e.g., due to a
  `LoadCredential=` failure) blocks `wait_for_unit` with no useful error.
  `wait_until_succeeds(..., timeout=120)` wrapped in a `try`/`except`
  that dumps `systemctl status` + the unit journal gives you an
  informative failure instead of an opaque hang.

## Adding a new VM test

1. Create `modules/tests/_vm-fleet-scenarios/<name>.nix` following the
   `vm-fleet-tag-sync.nix` template.
2. Accept `mkCpNode`, `mkAgentNode`, `mkTlsCerts`, `testPrelude`, and
   `tlsCertsModule` via `scenarioArgs` (and `pkgs`, `lib`, etc. as needed
   with `...`).
3. Register the subtest in `modules/tests/vm-fleet-scenarios.nix`.
4. Add the check name to the `vm-fleet-*` section in the project README
   / CLAUDE.md commands block (automatic discovery means no script edit
   is needed).

For non-fleet VM tests (single-subsystem things like `vm-core` / `vm-infra`)
follow the pattern in `modules/tests/vm.nix` — use `mkTestNode` directly.

## Shared `/nix/store` and the assertion classes it forbids (WONTFIX)

Every node in a `nixosTest` mounts the host's `/nix/store` read-only via 9p.
This means store-path existence checks (`test -e /nix/store/...`) are
**tautologically true** on every node regardless of which node's closure
references the path. A `nix copy` between nodes appears to succeed even
when it transferred zero bytes, because the receiver could already see
the path via 9p.

Phase 3 settled on two workaround patterns instead of the heavy-weight
per-VM store-image approach:

| Need | Workaround | Why it works |
|---|---|---|
| Prove a command ran on a specific node | VM-local marker file under `/tmp` | `/tmp` is per-VM, never shared via 9p |
| Prove a path is registered in a node's Nix DB | `nix-store -q --references <path>` on the target | The Nix DB (`/nix/var/nix/db`) is per-VM, only the store *files* are shared |

Concrete examples in the suite:

- `vm-fleet-deploy-ssh` uses `nix-store -q --references` to prove
  `nix-copy-closure --to` actually registered the stub closure in the
  target's Nix DB. The 9p-mounted store would make a `test -e` check
  invariant.
- `vm-fleet-rollback-ssh` uses the same pattern for the per-generation
  rollback assertion.
- `vm-fleet-apply-failure` uses `/tmp/stub-switch-called` (a regular
  filesystem path, VM-local) as the load-bearing proof that
  `switch-to-configuration switch` was invoked.

### Why not per-VM store images

The alternative — `virtualisation.useNixStoreImage = true;
virtualisation.mountHostNixStore = false;` — was considered and rejected:
every node would rebuild its own store image, multiplying VM build cost
for an assertion class that the workarounds already cover. Phase 3 did
not surface a scenario where the workarounds were insufficient.

If a future scenario genuinely requires per-VM store isolation
(e.g. asserting on byte-level transfer through `nix copy` rather than
DB registration), revisit this decision in a follow-up plan. Do not
adopt per-VM store images preemptively — they cost real wall-clock
minutes per CI run.
