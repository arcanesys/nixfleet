# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/). Versioning: [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Per-host/tag/channel commit pins for fleet.resolved.json (2026-05-07)

Closes #88. Operators had no way to freeze a host (or a tag's worth of hosts, or an entire channel) on a specific commit while the rest of the fleet kept iterating — every push promoted every reachable host to the new closure. Now mkFleet accepts a `pin = { commit; reason; expiresAt? }` declaration at any of three levels with most-specific-wins resolution, and `nixfleet-release` honors the pin by building each affected host's closure from the pinned commit instead of the current release commit.

#### Added

- **`hostType.pin`, `tagType.pin`, `channelType.pin`** in `lib/mk-fleet.nix` — same `pinType` submodule (`commit; reason; expiresAt?`) at all three levels. `expiresAt` is RFC3339 / ISO-8601 string; `nixfleet-release` filters expired pins (chrono-based comparison; pure Nix has no robust date parsing, so the filter doesn't run at mkFleet eval time).
- **`resolvePin` helper** inside `resolveFleet` — walks host > tag > channel; emits the most-specific non-null pin to the host's `fleet.resolved.json` entry. Multi-tag conflict (host has 2+ tags whose pins both apply) is rejected eagerly in `checkInvariants` so test harnesses' `tryEval` catches the error without forcing the lazy `mapAttrs` thunk.
- **`Pin` struct + `Host.pin: Option<Pin>`** in `crates/nixfleet-proto/src/fleet_resolved.rs` — `commit`, `reason`, `expires_at`. Re-exported from `nixfleet_proto` top-level. `Host` literals across the workspace updated to default `pin: None`.
- **`nixfleet-release` pinned-build path** (`crates/nixfleet-release/src/lib.rs::build_pinned`) — when a host's `pin.commit ≠ current_release_commit`, builds via `nix build "<pin_source_url>?rev=<commit>#<prefix>.<host>.config.system.build.toplevel"`. Same-commit pins fall through to the existing local build path. Pipeline reordered: eval BEFORE build so pin metadata can drive the build path.
- **`--pin-source-url <URL>` CLI flag** on `nixfleet-release` — required iff any non-expired host pin specifies a commit different from the current release commit (validated AFTER eval so expired pins don't impose the requirement). Typical: `git+ssh://lab:222/abstracts33d/fleet`.
- **`filter_expired_pins(&mut FleetResolved, Utc::now())`** — drops `expires_at <= now` pins so affected hosts fall back to the current-commit build path. The filter runs once per release; the resulting `fleet.resolved.json` only carries non-expired pins.
- **`HostStatusEntry.pin: Option<Pin>`** + state_view passes through from the verified fleet snapshot.
- **CLI** `nixfleet status` appends `🔒<short-commit>` to whatever the host's existing label is (converged / failed / activating / etc.). Pin is operator metadata, not a status of its own — augmenting preserves the health signal.

#### Behavior

```nix
mkFleet {
  hosts.web-02.pin = {
    commit = "abc1234";
    reason = "investigating CVE-2026-...";
  };
  channels.stable.pin = {
    commit = "def5678";
    reason = "freeze for Q2 audit";
    expiresAt = "2026-06-01T00:00:00Z";
  };
  tags.infra.pin = {
    commit = "9876fed";
    reason = "lagging edge by 2 commits as policy";
  };
}
```

Resolution: web-02 takes its own pin (host wins); other hosts tagged `infra` take the tag pin; remaining stable hosts take the channel pin. Channel pin expires automatically on 2026-06-01.

#### Tests

- mkFleet harness: positive fixture asserting precedence (host overrides tag overrides channel); negative fixture asserting multi-tag conflict throws.
- `nixfleet-release` unit tests: `filter_expired_pins` (past / future / null / exact-now boundary), `pin_target_commit` (unpinned / matching / diverging), `validate_pin_source_url` (errors when needed and unset; OK when no pins, when pin matches current commit, or when URL is set).
- CLI: pin-suffix preserves base label on converged + failed paths; commit prefix truncated to 7 chars.

#### Notes

- The CI orchestration is nixfleet-side, not fleet-side: `nixfleet-release` lives in `crates/nixfleet-release/` and is the binary fleet repos invoke from their CI workflow. Pinned hosts therefore need NO additional fleet-side build logic — just the declaration in `fleet.nix` and a `--pin-source-url` flag passed to `nixfleet-release`.
- Out of scope: pinning to a closureHash directly (instead of a commit). Could be added later as `pin.closureHash` for operators who'd rather skip the build dance.
- Out of scope: structural validation of the pinned commit's existence (we let `nix build` fail loudly when the operator types a bogus rev).

### Fix: `wait_ssh` ignores `--identity-key`, hangs on password prompt (2026-05-07)

Closes #89. `nix run .#build-vm -- -h <host> --identity-key <path>` hung at the SSH-readiness phase asking for a password: `wait_ssh` invoked `ssh` without `-i`, so the readiness probe tried `~/.ssh/id_*` then fell back to interactive password auth — fatal for non-interactive runs and impossible to satisfy with the ISO key (which is only known to the operator who built the ISO via `nixfleet.isoSshKeys`).

#### Changed

- **`lib/vm-helpers.sh::wait_ssh`** now honors `${IDENTITY_KEY:-}` (set by `build-vm` and `test-vm` from `--identity-key`). When non-empty, adds `-i $IDENTITY_KEY -o IdentitiesOnly=yes` to the readiness probe so it uses the SAME key that's baked into the ISO.
- **`-o BatchMode=yes`** added unconditionally — defense-in-depth, makes the password-prompt symptom impossible regardless of key state. Fleets are keys-only by design (`nixfleet.isoSshKeys`); password auth was never a legitimate recovery path here.

#### Notes

- No changes to `build-vm` / `test-vm` script-level argument parsing — `IDENTITY_KEY` is already a script-scope variable that `wait_ssh` reads via dynamic scoping. One-line fix at the call site would also have worked but threading the convention through the helper keeps both scripts symmetric.

### Per-host VM port-forwards in `mkVmApps` (2026-05-07)

Closes #87. `start-vm` only forwarded SSH-on-22 (`hostfwd=tcp::SSH_PORT-:22`); reaching guest services from the host required either SSH-into-the-VM-and-curl-localhost or hand-rolled `--vlan` networking. Demo-style walkthroughs ("`curl http://localhost:2280/version`") couldn't be expressed declaratively.

#### Added

- **`hostSpec.vmPortForwards: attrsOf port`** in `contracts/host-spec.nix` — per-host map of guest-port (string key) → host-port (int value). Defaults to `{}`. Only consumed by `nixfleet start-vm` (the install + smoke-test scripts deliberately stay SSH-only since they're short-lived).
- **`compute_extra_hostfwd_args` helper** in `lib/vm-helpers.sh` — fetches the map via `nix eval ... --apply 'builtins.toJSON'`, parses it with a tiny sed pipe (no jq dependency), and emits `,hostfwd=tcp::HOST-:GUEST,...` segments that concatenate onto the existing -nic argument's hostfwd= chain. Silent fail-open on eval errors — the SSH forward always lands.
- **`lib/vm-scripts/start.nix`** invokes the helper before launching qemu and interpolates `$EXTRA_HOSTFWD_ARGS` after the SSH hostfwd.

#### Notes

- Out of scope: build-vm and test-vm (install + smoke-test paths) — both are short-lived and only need SSH-on-22; no operator UX win from forwarding more.
- Fleet-side / demo-side adoption is the consumer's responsibility (a separate work item per the `nixfleet-demo` walkthrough).

### Closure-hash quarantine on activation failure (2026-05-07)

Closes #55. The agent retried known-failing `closure_hash` values on every poll cycle without backoff or quarantine. Each retry burned a switch-to-configuration + rollback cycle, emitted churn (logs, IO), and gave the operator no signal distinguishing "transient hiccup" from "permanently broken release". Layered on top of #56's switch-inhibitor work, sharing the per-closure sentinel pattern.

#### Added

- **`ReportEvent::RolloutQuarantined { closure_hash, channel_ref, failure_count, reason }`** — additive wire variant. Discriminator: `rollout-quarantined`. Unsigned (operator-surface only, no fleet gate reads it).
- **`LastFailedClosureRecord`** in `crates/nixfleet-agent/src/checkin_state.rs` — single-record agent-side persistence: `closure_hash`, `channel_ref`, `last_failure_at`, `failure_count`, `reason`, `last_quarantine_post_at`. Auto-supersedes when a different `closure_hash` fails (count resets to 1).
- **`record_switch_failure(state_dir, closure_hash, channel_ref, reason, now)`** — increment-or-reset semantics. Called from `dispatch/verify_mismatch.rs::handle_switch_failed` and `handle_verify_mismatch`. Preserves `last_quarantine_post_at` across same-hash failures so the throttle window doesn't reset on every flap.
- **`crates/nixfleet-agent/src/dispatch/quarantined.rs`** — suppression handler. `evaluate(state_dir, target, now)` returns `Proceed` or `Suppress(record)` based on closure_hash match + 24h `QUARANTINE_WINDOW_SECS`. `post_quarantine_event` re-posts at most once per `QUARANTINE_REPOST_THROTTLE_SECS` (1h) to bound journal volume during steady-state quarantine.
- **`HostStatusEntry.quarantined_closure: Option<String>`** — set when the host has a `RolloutQuarantined` event for its current rollout in the event ring. Event-ring derived (NOT DB-backed): there's no CP-side state-machine entry for "quarantined" because the existing SwitchFailed → rollback flow already drives `host_dispatch_state` to RolledBack. Quarantine is purely an operator signal, and the event ring's eviction window roughly matches the 24h suppression window.
- **`nixfleet status`** shows `✗ quarantined` ahead of `⟳ pending reboot`, between `failed` and `pending reboot` in priority — quarantine requires CI-side intervention while pending-reboot is operator-recoverable on the host itself.

#### Behavior

When a closure fails activation (SwitchFailed or VerifyMismatch outcome):
1. The existing rollback fires; agent posts `ActivationFailed` + `RollbackTriggered`. CP marks the dispatch `RolledBack` via the existing `apply_rollback_state_transition` flow.
2. The agent records `last_failed_closure` in its state-dir (increment if same closure_hash, else reset).
3. On the next dispatch poll for the SAME closure_hash within 24h: agent's `evaluate` returns `Suppress(record)`. The dispatch loop short-circuits before `activate()` — no realise, no nix-env --set, no fire_switch — and posts `RolloutQuarantined`. Subsequent suppressions within the throttle hour are silent.
4. CI publishes a fix → channel-ref advances → new closure_hash on next dispatch → `evaluate` returns `Proceed` → activation runs normally. The stale `last_failed_closure` record sits inert until something matches it again or the next failure overwrites it.

#### Tests

- 6 unit tests for `LastFailedClosureRecord` persistence and `record_switch_failure` semantics (round-trip, increment-on-same-hash, reset-on-different-hash, throttle-timestamp preservation, idempotency).
- 4 dispatch handler tests for `evaluate`/`post_quarantine_event` (suppress when matching, proceed on different closure, proceed after window expires, throttle posts within 1h).
- CLI test pinning the priority order: `quarantined` outranks `pending reboot`.

#### Notes

- Composes cleanly with #56's deferred suppression: the dispatch loop checks deferred first, then quarantine. Both auto-clear on closure_hash advance via the same passive-supersession pattern.
- Out of scope: cross-host quarantine view (e.g. "this rollout is quarantined fleet-wide" derived from N quarantined hosts), automatic rollout cancellation when quarantine count exceeds a threshold. Both are dashboard-side concerns once we have the per-host signal.

### Switch-inhibitor carve-out for live activation (2026-05-07)

Closes #56. `nixos-rebuild switch` refuses to live-swap critical components (dbus implementation, systemd, kernel, init) on a running system because the swap can hang processes or require kernel cooperation. The fire-and-forget agent (ADR-011) bypassed `nixos-rebuild`'s wrapper, so the same swap the operator-side guard refuses was happening silently via the gitops loop.

#### Added

- **`detect_switch_inhibitors`** in `crates/nixfleet-agent/src/activation/linux.rs` — canonicalize-equality compare on four store-relative paths (`etc/systemd/system/dbus.service`, `sw/lib/systemd/systemd`, `kernel`, `init`) between `/run/current-system` and the new closure. Mismatch → live switch unsafe; defer to next boot.
- **`ActivationOutcome::DeferredPendingReboot { component }`** — distinct from `SwitchFailed`; profile is set, no rollback fires, boot-recovery confirms post-reboot.
- **`ReportEvent::ActivationDeferred { closure_hash, channel_ref, component }`** — additive wire variant, unsigned (observability-flavor matching `ActivationStarted`). Discriminator: `activation-deferred`.
- **`PendingConfirmState::DeferredPendingReboot`** — new variant on the `host_dispatch_state.state` SQL CHECK constraint (migration `V005__pending_confirms_deferred_state.sql`). The 360s rollback timer's partial index is `WHERE state = 'pending'` so deferred rows are naturally excluded — no special-case timer code path. The confirm endpoint accepts `(Pending AND deadline > now) OR DeferredPendingReboot` as valid pre-Confirmed states; post-reboot confirms succeed regardless of deadline (the deferred lifecycle is human-paced, not agent-paced).
- **`apply_deferred_pending_reboot_transition`** in `crates/nixfleet-control-plane/src/server/routes/reports.rs` — CP-side state-driving handler. On `ActivationDeferred` event receipt, calls `host_dispatch_state.mark_deferred(host, rollout)` to park the row (Pending → DeferredPendingReboot). Mirrors the existing `apply_rollback_state_transition` shape.
- **`HostStatusEntry.pendingReboot: bool`** — set when the host's `host_dispatch_state` row is `DeferredPendingReboot`. **DB-backed**, not event-ring derived: durable across CP restart, single source of truth, doesn't depend on the in-memory ring's eviction policy. Cleared automatically when the row transitions to `Confirmed` (post-reboot retroactive confirm).
- **`nixfleet status`** shows `⟳ pending reboot` ahead of `✓ converged`, between `failed` and `stale` in priority.
- **Agent state-dir `last_deferred` sentinel** (`crates/nixfleet-agent/src/checkin_state.rs::LastDeferredRecord`) — written by `handle_deferred_pending_reboot`. Suppresses redundant activate-and-defer cycles: the dispatch loop short-circuits before `activate()` when the next target's `closure_hash` matches the recorded value, so re-posts of `ActivationDeferred` are O(1) per closure rather than O(poll-interval) until reboot. Cleared by `record_confirm_success` on both the live-switch and boot-recovery paths.

#### Behavior

When a deploy hits a switch-inhibitor: agent runs `nix-env --profile … --set <store_path>` (bootloader entry written for the new gen), skips `systemd-run --unit=nixfleet-switch`, and posts `ActivationDeferred`. CP parks the dispatch row in `DeferredPendingReboot`; the rollback timer's 360s sweep skips it. Operator sees `pendingReboot: true` in `/v1/hosts`. After the operator reboots — at any point, hours or days later — boot-recovery POSTs confirm; CP's confirm endpoint accepts the deferred row without the deadline gate and transitions it to `Confirmed`. Wave promotion / channel edges / disruption budget all see the deferred host as `ConfirmWindow` (in-flight, not terminal-for-ordering), so successor waves and channel-edge crossings correctly wait for the reboot.

#### Out of scope

- Glibc major-version swaps (requires walking `<store>/sw/lib/libc.so` symlink chain).
- `boot.loader.systemd-boot` ↔ `grub` swaps (post-activation hook, not pre-switch).
- Operator override flag for ops who want to opt out.
- Long-window escalation (e.g. alarm if the row has been deferred >7 days). Operator is responsible for rebooting; CP refuses to time out the lifecycle, but does not yet escalate.

#### Tests

- 4 unit tests for `detect_switch_inhibitors` (identical, dbus-differs, kernel-differs, missing-path).
- Dispatch handler test asserts `ActivationDeferred` payload + no rollback.
- CLI test asserts `⟳ pending reboot` priority.
- Existing `outcome_kinds_are_distinct` and `discriminator_matches_serde_event_tag` extended for the new variants.

#### Notes

- ADR-011's fire-and-forget invariant is preserved for non-inhibited switches. CONTRACTS.md §I.7 documents the carve-out as a sub-section.
- A NixOS VM harness scenario (`tests/harness/scenarios/switch-inhibitor.nix`) is the natural follow-up for end-to-end coverage.

### Cross-channel rollout ordering + tag-driven disruption budgets (2026-05-04)

Closes RFC-0002 §4.3's cross-channel coordination punt (#65). Two coordinated changes shipped together because both move budget/edge resolution from fleet-eval time to reconcile time.

#### Added

- **`fleet.channelEdges = [{ before; after; reason }]`** — DAG ordering between channels. The reconciler refuses to OpenRollout for `after` while `before` has any non-terminal rollout. mkFleet validates: both channels must exist, no cycles (reuses `hasCycle`), `before != after`. RFC-0002 §4.3 punt resolved as: if `before` has never had a rollout, the gate is open (proceed). `Halted` predecessor blocks `after` — operator must clear the halt or remove the edge.
- **`Action::RolloutDeferred { channel, target_ref, blocked_by, reason }`** — emitted when a channelEdge holds OpenRollout. Debounced via `Observed.last_deferrals`: same `(target_ref, blocked_by)` doesn't re-fire across reconcile ticks. CP `apply_actions` stamps the in-memory `last_deferrals_emitted` map on emit and clears on `OpenRollout`, feeding it back into the next tick's projection.

#### Changed

- **Disruption budgets are tag-driven at the wire level.** `disruptionBudgets[].selector: Selector` replaces the previously-eval-expanded `hosts: [..]` field. The reconciler resolves selectors at lookup time, so adding/removing a tagged host (e.g. retagging `ohm` from `family` to `dev`) takes effect on the next reconcile tick without re-signing fleet.resolved. **Hard schema cutover** — pre-feat-channel-edges artifacts (`hosts: [..]`) no longer parse; the CP must rebuild on a release CI'd with this version. Operators upgrading should also wipe the CP's state.db so no in-flight rollout state from the old schema lingers.
- **`Selector::matches(host_name, host)`** + `resolve()` — promoted from internal-to-Nix to a runtime helper on the proto type. Mirrors `lib/mk-fleet.nix:resolveSelector`.

#### Tests

- **Reconciler unit tests** for the new branch: predecessor active blocks, no-history proceeds, debounce holds across ticks, blocker-change re-fires, predecessor-cleared opens. Plus a budget test asserting tag-driven selectors resolve at call time.

#### Notes

- **Wave sequencing was already correct.** Investigation into "waves fire simultaneously" found `current_wave`-gated dispatch (`host_state.rs:242`) and `wave_all_soaked` promotion (`rollout_state.rs:81-140`). The previous symptom was a single 3-host `workstation`-tagged wave serialized only by `maxInFlight=1`; not a sequencing bug.
- **Schema is wire-breaking for `disruptionBudgets`.** `channelEdges:[]` is additive (matches the existing `edges:[]` convention); proto goldens updated to include the empty list. `disruptionBudgets[].selector` is required — old artifacts emitting `hosts:[..]` will fail to deserialize. CP and agent must be on the same nixfleet rev as the producing CI for a release to be consumable.

### v0.2 acceptance cycle (2026-04-30)

ARCHITECTURE.md §8's four falsifiable done-criteria are now harness-enforced end-to-end. Closes the gap from "stated as a contract" to "fails loudly on regression." Net −2,421 LOC across 83 commits; 280 Rust tests, 0 clippy warnings, 9 microvm scenarios.

#### Added — harness scenarios

- **`fleet-harness-corruption-rejection`** (§8 #4 — corrupted-artifact rejection). Pure runCommand check: bit-flips canonical bytes and signature in turn against `nixfleet-verify-artifact`, asserts each is rejected with the typed `VerifyError`.
- **`fleet-harness-auditor-chain`** (§8 #2 — offline auditor chain). Demonstrates `nixfleet-verify-artifact probe` accepts a well-formed signed compliance payload and rejects a byte-flipped copy. Verifies the host↔probes link without CP access.
- **`fleet-harness-secret-hygiene`** (§8 #3 — zero plaintext on stolen CP disk). Agent decrypts an age-encrypted blob at boot, lands plaintext in `/run/secrets/test-token`, then runs through normal checkin traffic; testScript greps the CP's state.db, journal, audit.log, and `/etc/nixfleet-cp/` tree for the plaintext, asserts no leaks.
- **`fleet-harness-teardown`** extended (§8 #1 — CP rebuild within one reconcile cycle). Beyond the prior soft-state checkin replay: now also asserts the signed `revocations.json` sidecar replays into `cert_revocations` post-wipe, and the agent-attested `last_confirmed_at` repopulates `host_rollout_state.last_healthy_since` via `recover_soak_state_from_attestation`. The fixture injects per-host `closureHash` and the agent VM overrides `/run/current-system` so convergence triggers the recovery path. Closes #14.

#### Added — supporting infrastructure

- **Shared `signBytes` helper** (`tests/harness/fixtures/signed/sign-bytes.nix`) factors the JCS+ed25519 signing path. Used by the main signed fixture and by new sidecar fixtures (revocations, probe outputs).
- **`nixfleet_reconciler::evidence`** consolidates probe-output verify (moved from `nixfleet-control-plane`'s `evidence_verify` module). Both CP and the offline `nixfleet-verify-artifact` CLI share one implementation.
- **`nixfleet-verify-artifact probe` subcommand** for offline audit verification (canonical-bytes + base64 signature + OpenSSH ed25519 pubkey → exit 0/1).
- **Probe-output fixture** (`tests/harness/fixtures/probe/`) bakes a signed `ComplianceFailureSignedPayload` for the auditor scenario.
- **Revocations fixture** (`tests/harness/fixtures/signed/revocations.nix`) bakes a signed `Revocations` envelope for the teardown scenario.
- **Agenix fixture** (`tests/harness/fixtures/agenix/`) provides a deterministic age identity + encrypted-secret pair for the secret-hygiene scenario.
- **Flake-check registration** for the new fixtures (`signed-fixture`, `probe-fixture`, `revocations-fixture`) — byte-stability regression guards.

#### Changed

- **Tracking-cycle nomenclature scrub.** `Phase N` / `criterion #N` / `gap A` / `phase-2-signed-fixture` and similar labels removed from source code, flake check names, and reference docs. Code reads timeless; tracking lives in GitHub issues. Renamed checks: `phase-2-signed-fixture` → `signed-fixture`, `phase-1-2-probe-fixture` → `probe-fixture`. CHANGELOG entries (this file) are exempt — dated record genre.
- **Bare GitHub-issue refs scrub from source.** `(#46)`, `(#48)`, `closes #N` style references stripped from Rust + Nix sources (28 files, no net LOC change). Substantive descriptions retained; commit messages and CHANGELOG entries keep the refs.
- **Markdown cleanup (5-phase pass, −10,636 LOC).** Deleted `docs/superpowers/`, `docs/KICKOFF.md`, all `phase-N-entry-spec.md` files, and the `docs/roadmap/` tracking files; tracking content migrated to issues #67/#68/#69. Reference docs (ARCHITECTURE.md, CONTRACTS.md, RFCs, DISASTER-RECOVERY.md) compacted; "implementation status (date)" blocks removed from RFC headers. `docs/README.md` rewritten to match actual on-disk structure.

#### Issues

- Closed: #14 (Phase 10 teardown test), #46 (orphan-confirm recovery), #47 (last_confirmed_at attestation), #48 (signed revocations sidecar), #57 (runtime compliance gate, agent), #58 (static compliance unification), #60 (host_reports SQLite). Plus quick-wins #49, #50, #52, #53, #54.
- Filed: #67 (pluggable activation backend, v0.3 scope), #68 (CheckinResponse.target widen for RFC-0003 §4.1), #69 (onHealthFailure rollback emission for RFC-0002 §5.1).
- Updated with progress: #4 (compliance gate umbrella; CLI surfacing → #66), #12 (signed artifacts umbrella; root-3 → #61, rotation → #63), #59 (CP-side wave-promotion gating; CLI surfacing → #66), #61 (probe signatures on remaining 6 activation-evidence variants).

#### Cycle scaffolding

- Memory rules captured: heavy builds run on lab not darwin; tracking-cycle labels stay out of code; microvm guests aren't first-class test driver nodes; testScript runs through mypy `--strict`; `_` is a real variable in tuple unpacks. Prevents re-learning the same lessons next cycle.

### v0.2 completeness cycle (2026-04-28)

Closes the framework-scoped gaps required for ARCHITECTURE.md §8 done-criterion #1 — *"destroying the CP's database and rebuilding from empty state results in full fleet visibility within one reconcile cycle"* — to hold against strict reading. Six commits between `fe3baec` and `ac5a66f`; tests 127 → 165.

#### Added

- **Wave soak timer (RFC-0002 §3.2 Healthy → Soaked).**
  - `Action::SoakHost { rollout, host }` variant on the reconciler's action stream.
  - Reconciler `Healthy` arm consults `rollout.last_healthy_since[host]` against `wave.soak_minutes`; emits `SoakHost` when `now - last_healthy_since >= soak_window`.
  - CP-side `host_rollout_state` table (V003 migration) keyed on `(rollout_id, hostname)` with `host_state` + `last_healthy_since` columns.
  - DB methods: `record_host_healthy`, `clear_host_healthy`, `host_soak_state_for_rollout`, `healthy_rollouts_for_host`, `mark_host_soaked`, `host_rollout_state_exists`.
  - CP-side action processor in `server::reconcile::apply_actions` runs each tick to fold `SoakHost` actions into the DB.
  - `Rollout` widened with `last_healthy_since: HashMap<String, DateTime<Utc>>` (additive, `#[serde(default)]` keeps file-backed `observed.json` fixtures parseable).
  - `db::active_rollouts_snapshot` joins `pending_confirms` (latest per host, state ∈ `{pending, confirmed}`) with `host_rollout_state` so `observed_projection::project` populates `active_rollouts` (was hardcoded `Vec::new()` pre-cycle).

- **Confirm-handler idempotency (gap A, #46).** `/v1/agent/confirm` with no matching pending row now cross-checks the agent's `closure_hash` against the verified target; match → synthetic `confirmed` row + `record_host_healthy` + 204. Mismatch → 410 (existing semantics). Closes the unnecessary-rollback regression on CP rebuild.

- **Signed `revocations.json` sidecar (gap C, #48).** New CONTRACTS.md §I artifact alongside `fleet.resolved.json`, signed by the same `ciReleaseKey`. CP fetches + verifies + replays into `cert_revocations` on every reconcile tick. Operator UX shifts revocations from CLI-on-CP to git commit + CI sign + push. Closes the only security-material rebuild gap.
  - New types: `nixfleet_proto::Revocations` + `RevocationEntry`.
  - New verify path: `nixfleet_reconciler::verify_revocations`.
  - New CP poll: `revocations_poll` module + `--revocations-artifact-url` / `--revocations-signature-url` / `--revocations-token-file` CLI flags.
  - Release-tool integration: optional `--revocations-attr <attr>` flag signs the operator-declared list alongside `fleet.resolved.json`.
  - Nix-side: `mkFleet` gains a `revocations` option; surfaced as `<flake>.fleet.revocations`.

- **Agent-attested `last_confirmed_at` (gap B-cp, #47 — CP-side half).** New optional field on `CheckinRequest` (wire-additive, no protocol bump). CP repopulates `host_rollout_state.last_healthy_since` from the attestation when the host is converged on its target with no existing `host_rollout_state` row, clamped to `min(now, attested)` against clock skew. Agent-side population (B-agent) folds into #2 when the agent activation loop lands.

- **`signed_fetch` module.** Shared `build_client` / `read_token` / `fetch_signed_pair` helpers extracted from `channel_refs_poll` + `revocations_poll` so the two parallel modules stay byte-stable on the HTTP fetch path.

- **End-to-end soak-loop integration test (`tests/soak_loop.rs`).** Single test exercises the full chain: `confirm` → `record_healthy` → projection → reconciler → `SoakHost` → `mark_soaked` → projection → `ConvergeRollout`.

#### Documentation

- **`docs/commercial-extensions.md`** (new). Catalogues capabilities the open kernel intentionally does not ship — HA replication, real-time signed-state snapshots, SLA observability, audit packages, hosted CP, multi-tenant federation, fine-grained RBAC, long-running metrics warehousing — with stranger-fleet-test rationale and integration paths.
- **ARCHITECTURE.md §6 Phase 10 — "CP-resident state by recovery profile"** subsection enumerating every SQLite table with its recovery class (soft from agent inputs / hard from signed artifacts in git).
- **ARCHITECTURE.md §7 Non-goals** points at `docs/commercial-extensions.md` for capabilities deliberately out of scope.
- **ARCHITECTURE.md §8 done-criterion #1** expanded with the per-table guarantee.
- **v0.2 completeness cycle landed** — gap #2 closed (steps 1+2+3); gaps A/B/C/D enumerated with their closing commits. Tracking moved to GitHub issues (#46/#47/#48/#14, plus open #68/#69/#67 for the remaining items).

#### Issues

- Closed: #46 (gap A), #48 (gap C).
- Updated: #47 (gap B — CP-side complete, agent-side defers to #2), #14 (Phase 10 teardown — acceptance criterion refreshed; microvm.nix scenario deferred to next cycle pending #5's harness work), #10 (v0.2 tracking — cycle summary), #12 (signed artifacts — cross-link to gap C), #2 (Magic rollback — naming the slot for B-agent).

### Architecture refactor — kernel/opinion split (2026-04-27 → 2026-04-28)

Two-repo architecture: framework + consumer fleet. `nixfleet-scopes` archived; its
contents folded into `nixfleet` (contract impls) and the consuming fleet
(service wraps, role bundles, hardware modules, platform shims).

#### Added

- **`contracts/`** (top-level) — schemas: `host-spec.nix`, `trust.nix`, `persistence.nix`. Moved out of `modules/` because import-tree treats `modules/` as flake-parts modules and the schemas' `assertions` declarations leak into flake-parts level if put inside.
- **`impls/`** (top-level) — pluggable contract impls absorbed from former `nixfleet-scopes`:
  - `impls/persistence/impermanence.nix` — btrfs root-wipe + impermanence module wiring. New options: `nixfleet.persistence.impermanence.{rootDevice, oldRootsRetentionDays}`.
  - `impls/keyslots/tpm/` — TPM-backed signing keyslot.
  - `impls/gitops/forgejo.nix` — channel-refs URL builder for Forgejo / Gitea.
  - `impls/secrets/default.nix` — backend-agnostic identity-path resolution.
- **`flake.scopes.<family>.<impl>`** — new public output exposing contract impls. Example: `inputs.nixfleet.scopes.persistence.impermanence`.
- **`impermanence`** flake input (required by `impls/persistence/impermanence.nix`; inert when that impl is not imported).

#### Changed

- **`lib/` consolidation.** `modules/_shared/lib/` collapsed into top-level `lib/`. Single entry: `lib/default.nix` is the wired entry (`{inputs, lib}`). `lib/mk-fleet.nix` is the pure entry (`{lib}`-only) for the canonicalize binary and eval-only tests.
- **File naming standardised** to kebab-case across the framework:
  - `lib/mkFleet.nix` → `lib/mk-fleet.nix` (function `mkFleet` unchanged).
  - `tests/lib/mkFleet/` → `tests/lib/mk-fleet/`.
  - `modules/scopes/nixfleet/_agent_darwin.nix` → `_agent-darwin.nix`.
- **Schemas relocated** to `contracts/` and renamed to drop the redundant `-module` suffix:
  - `modules/_trust.nix` → `contracts/trust.nix`.
  - `modules/_shared/host-spec-module.nix` → `contracts/host-spec.nix`.
  - `modules/scopes/nixfleet/_persistence.nix` → `contracts/persistence.nix`.
- **Framework `core/_*.nix` trimmed to true prerequisites only.** `_nixos.nix` keeps trust import + flake-mode `nix` settings + `hostSpec` → standard NixOS option pass-through + root SSH from `hostSpec`. `_darwin.nix` keeps `system.stateVersion`, `system.checks.verifyNixPath`, `system.primaryUser`, `hostSpec.isDarwin`. The opinions that used to ship from these (substituter lists, GC policy, openssh hardening, nixpkgs.config defaults, network baselines, Dock management, Determinate-Nix wiring, TouchID + pam-reattach) are now consumer-fleet responsibility.
- **Opinion-leak audit on docstrings, comments, and option examples.** `lab.internal` / `abstracts33d` / `krach` / `s33d` replaced with neutral examples (`example.com` / `myorg` / `test-host`); `/run/agenix/*` examples replaced with `/run/secrets/*` so the framework reads file paths backend-agnostically; `attic push fleet ...` typical-example expanded to list cache-server alternatives.
- **`secrets.identityPaths.userKey` default** changed from `${hS.home}/.keys/id_ed25519` to `${hS.home}/.ssh/id_ed25519` (universal NixOS / userland convention).
- **`rfcs/`** moved to **`docs/rfcs/`**. Doc-generation in `modules/rust-packages.nix` reads from the new location.
- **`flake.lib`** is now the wired entry; consumers that previously read `inputs.nixfleet.scopes.X` from `nixfleet-scopes` now read `inputs.nixfleet.scopes.X` from this repo (same attribute path, different source).

#### Removed (public surface)

- **`flake.diskoTemplates.*`** — disk templates dropped from public output. `nixfleet`'s QEMU test fixture keeps a co-located template at `tests/fixtures/qemu/disk-template.nix`. Consuming fleets carry their own templates.
- **`flakeModules.{iso, formatter, apps, tests}`** — fleet repos that imported the framework's iso / formatter / apps / tests perSystem modules now host their own.
- **`modules/iso.nix`** and **`modules/formatter.nix`** — consumers absorb these locally.
- **`modules/_hardware/qemu/`** — moved to `tests/fixtures/qemu/` (clearly scoped to framework-internal test harness, not a public output).

#### Earlier in the cycle (still under [Unreleased] from before this refactor)

- `lib.mkFleet` — evaluates a declarative fleet description per RFC-0001 and emits a typed `.resolved` artifact. Every invariant from §4.2 is enforced at eval time: host/channel/policy references, host `configuration` validity, edge DAG, compliance-framework allow-list, and the cross-field `freshnessWindow ≥ 2 × signingIntervalMinutes` relation.
- `lib.withSignature` — helper that CI calls to stamp `meta.signedAt` / `meta.ciCommit` onto a resolved fleet before signing.
- `nixfleet.trust.*` option tree (now at `contracts/trust.nix`) — declares CI release key, attic cache key, and org root key (with rotation grace slots and a compromise `rejectBefore` switch) per `docs/CONTRACTS.md §II`.
- `tests/lib/mk-fleet/` (renamed from `tests/lib/mkFleet/`) — eval-only harness with positive fixtures (golden JSON comparison), negative fixtures (expected-failure via `tryEval`), and `_`-prefix filter for shared helpers.
- New channel options: `signingIntervalMinutes` (default 60) and `freshnessWindow` (no default — must declare). Existing channel definitions must add these to evaluate.
- New host option: `pubkey` (nullable, OpenSSH-format ed25519). Host entries may still omit it; enrollment-bound hosts MUST set it.
- `fleet.resolved` shape extended with a `meta` attribute (`{schemaVersion, signedAt, ciCommit}`) per `docs/CONTRACTS.md §I #1`. Top-level `schemaVersion: 1` is preserved for RFC-0001 §4.1 backward reference.

## [0.1.0] - 2026-04-19

Initial release.

[Unreleased]: https://github.com/arcanesys/nixfleet/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/arcanesys/nixfleet/releases/tag/v0.1.0
