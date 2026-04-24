# Stream C Milestone 1 Part 3 — v0.2 Agent + CP + CLI Skeletons Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the v0.2 skeleton binaries (`nixfleet-agent`, `nixfleet-control-plane`, `nixfleet-cli`) that close out Stream C Checkpoint 1 per `docs/KICKOFF.md §1 Stream C` — deliverables #3, #4, #5 — while trimming the now-unused v0.1 surface so the tree ends up on v0.2 only.

**Architecture:** Four stacked PRs on `abstracts33d/nixfleet`, branched from today's `main` (`217de72`). PR 1 is the migration: delete v0.1 crates + tests + darwin module, extend `nixfleet-proto` with `TrustConfig`, extend `nixfleet-reconciler::verify_artifact` with `reject_before`, scaffold empty v0.2 crates that print a stub line and compile, rewrite the two scope modules + `modules/fleet.nix` to the v0.2 surface. PRs 2/3/4 fill in the CP, agent, and CLI bodies against the stable scaffold. Throughout: `verify_artifact` on every load, mTLS with CN-vs-hostname enforcement, SQLite columns with `-- derivable from:` annotations, restart-only trust reload, poll-only agent (no activation).

**Tech Stack:** Rust 2021 • axum 0.8 + axum-server (rustls) + tokio • rusqlite 0.32 + refinery 0.8 • reqwest 0.12 (rustls-tls) • clap 4 • ed25519-dalek 2 + p256 0.13 (already transitive via reconciler) • x509-parser 0.16 for CN extraction • chrono serde • Nix (alejandra-formatted) modules on the Nix side.

---

## Context — what is already in main

Landed in the last wave (commits `4ed26fd..217de72`):

- `docs/CONTRACTS.md` — §I #1 (fleet.resolved), §I #2 (wire protocol), §II (trust roots), §III (JCS canonicalization), §IV (CP storage purity rule).
- `docs/trust-root-flow.md` (PR #25) — authoritative for `TrustConfig` shape (§3.4), trust file placement (§3.2), rotation semantics (§6), decisions (§7).
- `docs/phase-2-entry-spec.md` (PR #28) — pins §12 decisions; lands AFTER this PR series.
- `tests/harness/*` (PR #27) — v0.2 microvm harness scaffold. Contains `TODO(5)` slot-in markers at `tests/harness/nodes/{cp,agent}.nix` that Phase 2 PR (b) will wire to the real services. **Do not touch `tests/harness/` in this PR series.**
- `crates/nixfleet-canonicalize` (flake-exposed in PR #26), `crates/nixfleet-proto`, `crates/nixfleet-reconciler` — all already at 0.2.0.
- `modules/_trust.nix` — typed `nixfleet.trust.{ciReleaseKey,atticCacheKey,orgRootKey}` tree with `current`/`previous` + `rejectBefore` + ed25519/ecdsa-p256 algorithm enum.

Issue to close: `abstracts33d/nixfleet#29`.

---

## Contract deltas to implement in this series (authoritative)

From `docs/trust-root-flow.md §7` + the coordinator's context update:

1. **`proto::TrustConfig`** — new struct with **required** top-level `schemaVersion: u32` (serde rename `"schemaVersion"`). Binaries refuse to start if `schemaVersion != 1`, returning a clear error.
2. **`proto::KeySlot::active_keys()`** signature is `fn(&self) -> Vec<TrustedPubkey>`. No `now` argument. Does not filter on `rejectBefore`. Both `current` and `previous` go into the returned Vec unconditionally.
3. **`reconciler::verify_artifact`** gains `reject_before: Option<DateTime<Utc>>` as a new parameter (alongside `freshness_window`). On signature-verify success, if `signed_at < reject_before`, return `VerifyError::RejectedBeforeTimestamp { signed_at, reject_before }` — a **new** variant, not `Stale`.
4. Both CP and agent binaries take `--trust-file <path>` CLI flag. Default paths `/etc/nixfleet/cp/trust.json` and `/etc/nixfleet/agent/trust.json`.
5. Reload model is **restart-only**. No SIGHUP, no inotify, no periodic re-read. NixOS `nixos-rebuild switch` triggers the restart when `environment.etc."nixfleet/*.trust.json"` content changes.

These are PR 1's work.

---

## File structure

### Delete (PR 1)

- `crates/agent/` — v0.1 agent crate (all src, tests, Cargo.toml).
- `crates/control-plane/` — v0.1 CP crate.
- `crates/cli/` — v0.1 CLI crate.
- `crates/shared/` — `nixfleet-types`; only consumed by the three v0.1 crates above.
- `modules/scopes/nixfleet/_agent_darwin.nix` — v0.1 darwin launchd daemon.
- `modules/tests/_vm-fleet-scenarios/` — all 12 files. Test v0.1 activation / retry / rollback behaviour that v0.2 deliberately does not have.
- `modules/tests/_lib/helpers.nix`, `modules/tests/_lib/nix-shim.nix` — helpers for the scenarios above.
- `modules/tests/vm-infra.nix` — v0.1 VM test entrypoints.

### Rewrite in place (PR 1)

- `modules/scopes/nixfleet/_agent.nix` — Linux-only v0.2 module. Writes `/etc/nixfleet/agent/trust.json` from `config.nixfleet.trust` via `environment.etc`. Calls `${pkgs.nixfleet-agent}/bin/nixfleet-agent --trust-file /etc/nixfleet/agent/trust.json --control-plane-url <…> …`. Drops v0.1 `healthChecks`, `tags`, `dryRun`, `allowInsecure` option trees.
- `modules/scopes/nixfleet/_control-plane.nix` — same pattern. Adds `--trust-file /etc/nixfleet/cp/trust.json` and `--release-path /var/lib/nixfleet-cp/fleet.git/releases/fleet.resolved.json`.
- `modules/fleet.nix` — `agent-test` and `agent-darwin-test` host configs simplified to the v0.2 surface; `agent-darwin-test` deleted (darwin agent is gone).
- `modules/tests/eval.nix` — drop all `launchd.daemons.nixfleet-agent` checks; keep linux-side option assertions where they still apply to v0.2.
- `crane-workspace.nix` — rewrite the four `buildPackage` blocks to point at the new crate paths, bump `version = "0.2.0"`, drop the `crates/shared` inclusion from `fileSetForCrate`.

### Extend (PR 1)

- `crates/nixfleet-proto/src/lib.rs`, `crates/nixfleet-proto/src/trust.rs` — add `TrustConfig`, `KeySlot`, `AtticKeySlot`.
- `crates/nixfleet-proto/src/wire.rs` — new file, wire-protocol types per RFC-0003 §4.1–4.3 (`CheckinRequest`, `CheckinResponse`, `ConfirmRequest`, `ReportRequest`, `Target`, `Health`, `CurrentGeneration`).
- `crates/nixfleet-reconciler/src/verify.rs` — add `reject_before` param + `RejectedBeforeTimestamp` variant.
- `crates/nixfleet-reconciler/tests/verify.rs` — add tests for the new variant and the slot-level semantics.

### Create (PR 1 scaffold, PRs 2/3/4 fill in)

- `crates/nixfleet-agent/Cargo.toml`, `crates/nixfleet-agent/src/main.rs`, `crates/nixfleet-agent/src/lib.rs`.
- `crates/nixfleet-control-plane/Cargo.toml`, `crates/nixfleet-control-plane/src/main.rs`, `crates/nixfleet-control-plane/src/lib.rs`, `crates/nixfleet-control-plane/migrations/V1__initial.sql`.
- `crates/nixfleet-cli/Cargo.toml`, `crates/nixfleet-cli/src/main.rs`, `crates/nixfleet-cli/src/lib.rs`.

### PR 2/3/4 extensions

- CP (PR 2): `src/cli.rs`, `src/state.rs`, `src/db.rs`, `src/tls.rs`, `src/routes/mod.rs`, `src/routes/checkin.rs`, `src/routes/confirm.rs`, `src/routes/report.rs`, `src/routes/closure.rs`, `src/release.rs`, `src/reconcile.rs`, `migrations/V1__initial.sql`. Tests at `crates/nixfleet-control-plane/tests/`.
- Agent (PR 3): `src/cli.rs`, `src/config.rs`, `src/enroll.rs`, `src/checkin.rs`, `src/fetch.rs`, `src/tls.rs`, `src/run.rs`. Tests at `crates/nixfleet-agent/tests/`.
- CLI (PR 4): `src/cli.rs`, `src/client.rs`, `src/status.rs`, `src/rollout.rs`. Tests at `crates/nixfleet-cli/tests/`.

---

## PR 1 — Migration + Foundation

Branch `feat/29-m1p3-migration`. Title: `feat(#29): v0.2 migration — trim v0.1, extend proto TrustConfig, reconciler reject_before, scaffold skeletons`.

### Task 1.1 — Create PR 1 branch

**Files:** none (branch state).

- [ ] **Step 1: Confirm worktree base**

```bash
cd /home/s33d/dev/arcanesys/nixfleet/.worktrees/stream-c-m1p3
git status
git log --oneline -3
```

Expected: clean tree on `feat/29-m1p3-skeletons`, latest commit `217de72 Merge pull request #28…`.

- [ ] **Step 2: Create PR 1 branch**

```bash
git checkout -b feat/29-m1p3-migration
```

### Task 1.2 — Delete v0.1 crates

**Files:** delete `crates/agent/`, `crates/control-plane/`, `crates/cli/`, `crates/shared/`.

- [ ] **Step 1: Delete the four v0.1 crate trees**

```bash
git rm -r crates/agent crates/control-plane crates/cli crates/shared
```

- [ ] **Step 2: Verify workspace glob still resolves**

```bash
cargo metadata --format-version 1 --no-deps 2>&1 | head -20
```

Expected: no error. Workspace members drop to `nixfleet-canonicalize`, `nixfleet-proto`, `nixfleet-reconciler`.

- [ ] **Step 3: Commit**

```bash
git commit -m "refactor(#29): delete v0.1 agent, control-plane, cli, shared crates

v0.1 is retired in favour of the v0.2 skeletons landing in this PR
series. v0.1 was the activation/magic-rollback iteration that Stream A's
lab masked (see abstracts33d/fleet#24); v0.2 is poll-only and consumes
the new boundary contracts (CONTRACTS.md §I/§II).

No feature migration: every v0.1 capability we still want (poll, report,
enrol, verify) is re-expressed against the v0.2 contract surface."
```

### Task 1.3 — Delete v0.1 VM test infrastructure

**Files:** delete `modules/tests/_vm-fleet-scenarios/`, `modules/tests/_lib/helpers.nix`, `modules/tests/_lib/nix-shim.nix`, `modules/tests/vm-infra.nix`.

- [ ] **Step 1: Delete**

```bash
git rm -r modules/tests/_vm-fleet-scenarios
git rm modules/tests/_lib/helpers.nix modules/tests/_lib/nix-shim.nix modules/tests/vm-infra.nix
# If _lib/ becomes empty, remove it too:
[ -d modules/tests/_lib ] && [ -z "$(ls modules/tests/_lib)" ] && rmdir modules/tests/_lib
```

- [ ] **Step 2: Check for dangling references**

```bash
grep -rn "_vm-fleet-scenarios\|_lib/helpers\|_lib/nix-shim\|vm-infra" modules/ flake.nix 2>&1 | head
```

Expected: no hits. Investigate and resolve if any.

- [ ] **Step 3: Commit**

```bash
git commit -m "refactor(#29): delete v0.1 VM test scenarios and helpers

The 12 scenario files under modules/tests/_vm-fleet-scenarios/ tested
v0.1 activation, retry, rollback, SSH-deploy behaviour that v0.2 does
not implement (poll-only, no activation; see RFC-0002 §3 + KICKOFF.md
§1 Stream C).

The v0.2 harness substrate lives at tests/harness/ (PR #27) and runs
TODO(5) stubs until the Phase 2 wire-up PR swaps in the new binaries."
```

### Task 1.4 — Delete darwin agent scope + v0.1 fleet.nix hosts

**Files:** delete `modules/scopes/nixfleet/_agent_darwin.nix`; edit `modules/fleet.nix`; edit `modules/tests/eval.nix`.

- [ ] **Step 1: Delete the darwin scope module**

```bash
git rm modules/scopes/nixfleet/_agent_darwin.nix
```

- [ ] **Step 2: Edit `modules/fleet.nix` — remove the `agent-darwin-test` host and simplify `agent-test`**

Locate the `agent-test` block (currently around lines 115–140) and replace with the v0.2 surface (enable + controlPlaneUrl; drop tags/healthChecks/metricsPort for the skeleton):

```nix
    # agent-test: exercises the v0.2 agent against a stub CP.
    agent-test = mkHost {
      hostName = "agent-test";
      platform = "x86_64-linux";
      isVm = true;
      hostSpec = orgDefaults;
      modules = [
        scopes.roles.workstation
        orgOperators
        {
          services.nixfleet-agent = {
            enable = true;
            controlPlaneUrl = "https://cp.test:8080";
          };
          nixfleet.trust.ciReleaseKey.current = {
            algorithm = "ed25519";
            public = "AAAA"; # eval-fixture placeholder; real host pins real key
          };
        }
      ];
    };
```

Locate the `aarch64-darwin` agent host block (currently around lines 240–270) and remove it entirely.

- [ ] **Step 3: Edit `modules/tests/eval.nix` — drop launchd agent assertions**

Find the `launchd.daemons.nixfleet-agent.*` checks (around lines 385–411) and delete them. Keep any v0.2-applicable option assertions (e.g. option type checks on `services.nixfleet-agent.enable`).

- [ ] **Step 4: Verify eval passes**

```bash
nix flake check --no-build --quiet 2>&1 | tail -30
```

Expected: no failure messages referencing `nixfleet-agent` launchd daemons or the deleted files. Genuine failures will appear — at this point the scope modules still call `pkgs.callPackage ../../../crates/agent {…}`, which is gone. Those are fixed in Task 1.7.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(#29): drop darwin agent module + v0.1 fleet.nix hosts

Darwin agent support is on the Phase 4 trim list. Dropping now; lab
already runs x86_64-linux + agenix for secrets. Simplified agent-test
to the v0.2 surface (controlPlaneUrl + trust declaration)."
```

### Task 1.5 — Extend `nixfleet-proto` with `TrustConfig` + patch `Channel` freshness-window unit

**Files:** modify `crates/nixfleet-proto/src/lib.rs`, `crates/nixfleet-proto/src/trust.rs`, `crates/nixfleet-proto/src/fleet_resolved.rs`; create `crates/nixfleet-proto/tests/trust_config.rs`; extend `crates/nixfleet-proto/tests/roundtrip.rs`.

**Latent unit landmine being fixed.** `crates/nixfleet-proto/src/fleet_resolved.rs:44` declares `pub freshness_window: u32` with no unit suffix. Sibling fields `signing_interval_minutes` and `reconcile_interval_minutes` are explicit. `lib/mkFleet.nix:120–127` docstring says "Minutes"; homelab fixture uses `180` (3 h) and `20160` (2 w). A reader who passes `chan.freshness_window` to `Duration::from_secs(_ as u64)` gets a 60× smaller window silently. Fix below adds a `Channel::freshness_window_duration()` helper and a doc comment so no caller trips. Lands in the same commit as `TrustConfig` (single-touch proto rule).

- [ ] **Step 1a: Write the failing test for `Channel::freshness_window_duration()` (unit landmine)**

Append to `crates/nixfleet-proto/tests/roundtrip.rs`:

```rust
#[test]
fn channel_freshness_window_duration_converts_minutes_to_seconds() {
    use std::time::Duration;
    // homelab 'stable' channel: 180-minute window = 3 hours = 10_800 seconds.
    let bytes = include_str!("fixtures/homelab-fleet-resolved.json");
    let fleet: nixfleet_proto::FleetResolved = serde_json::from_str(bytes).unwrap();
    let stable = fleet.channels.get("stable").expect("stable channel");
    assert_eq!(stable.freshness_window, 180, "fixture invariant");
    assert_eq!(
        stable.freshness_window_duration(),
        Duration::from_secs(10_800),
        "180 minutes converted to seconds"
    );
    // 'edge-slow': 20_160 minutes = 2 weeks.
    let edge = fleet.channels.get("edge-slow").expect("edge-slow channel");
    assert_eq!(
        edge.freshness_window_duration(),
        Duration::from_secs(20_160 * 60)
    );
}
```

If `crates/nixfleet-proto/tests/fixtures/homelab-fleet-resolved.json` does not exist, find the nearest existing fixture with these channels (likely created by PR #17/#19) and adjust the path. If none exists, create a minimal fixture inline in the test:

```rust
#[test]
fn channel_freshness_window_duration_converts_minutes_to_seconds() {
    use std::time::Duration;
    let json = r#"{
        "schemaVersion": 1,
        "meta": { "signedAt": null, "ciCommit": null, "signatureAlgorithm": null },
        "channels": {
            "stable": {
                "rolloutPolicy": "canary",
                "reconcileIntervalMinutes": 30,
                "freshnessWindow": 180,
                "signingIntervalMinutes": 60,
                "compliance": { "strict": true, "frameworks": [] }
            }
        },
        "hosts": {},
        "rolloutPolicies": {},
        "edges": [],
        "disruptionBudgets": {},
        "trust": {}
    }"#;
    let fleet: nixfleet_proto::FleetResolved = serde_json::from_str(json).unwrap();
    let stable = fleet.channels.get("stable").unwrap();
    assert_eq!(stable.freshness_window_duration(), Duration::from_secs(10_800));
}
```

Pick whichever shape matches the crate's existing fixtures — peek at `crates/nixfleet-proto/tests/` to decide.

- [ ] **Step 1b: Run test — expect compile failure on the missing helper**

```bash
cargo test -p nixfleet-proto --test roundtrip channel_freshness_window_duration 2>&1 | tail -10
```

Expected: compile error — `freshness_window_duration` does not exist.

- [ ] **Step 1c: Add doc comment + helper to `Channel`**

In `crates/nixfleet-proto/src/fleet_resolved.rs`, replace the `Channel` struct's `freshness_window` field declaration:

```rust
pub struct Channel {
    pub rollout_policy: String,
    pub reconcile_interval_minutes: u32,

    /// Minutes a signed `fleet.resolved` is accepted by consumers after
    /// `meta.signedAt`. Matches `lib/mkFleet.nix`'s declarative unit (the
    /// sibling `*_interval_minutes` fields make this pattern explicit
    /// there; the name here predates that convention and is kept for
    /// wire-compat — convert via [`Channel::freshness_window_duration`]).
    ///
    /// `lib/mkFleet.nix` enforces `freshness_window ≥ 2 × signing_interval_minutes`
    /// at eval time, so a value of `0` cannot reach the wire.
    pub freshness_window: u32,

    pub signing_interval_minutes: u32,
    pub compliance: Compliance,
}
```

And add an impl block just below the struct:

```rust
impl Channel {
    /// Returns `freshness_window` as a [`std::time::Duration`].
    ///
    /// The underlying field carries MINUTES (see the field doc); passing
    /// it directly to `Duration::from_secs` would silently shrink the
    /// window by 60×. Call this helper at the seam between proto and any
    /// `Duration`-consuming API (`verify_artifact`, tick handlers, …).
    pub fn freshness_window_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.freshness_window as u64 * 60)
    }
}
```

- [ ] **Step 1d: Run tests — confirm the unit helper passes + all existing round-trip tests stay green**

```bash
cargo test -p nixfleet-proto 2>&1 | tail -10
```

Expected: pre-existing roundtrip tests + the new unit test pass.

- [ ] **Step 2: Write the failing test for `TrustConfig` round-trip**

Create `crates/nixfleet-proto/tests/trust_config.rs`:

```rust
//! Round-trip tests for TrustConfig + KeySlot + AtticKeySlot.
//!
//! Shape authoritative per docs/trust-root-flow.md §3.4 + §7.4.

use nixfleet_proto::{AtticKeySlot, KeySlot, TrustConfig, TrustedPubkey};

#[test]
fn trust_config_roundtrips_minimum_shape() {
    let json = r#"{
        "schemaVersion": 1,
        "ciReleaseKey": {
            "current": { "algorithm": "ed25519", "public": "AAAA" },
            "previous": null,
            "rejectBefore": null
        },
        "atticCacheKey": null,
        "orgRootKey": null
    }"#;
    let cfg: TrustConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.schema_version, 1);
    assert_eq!(
        cfg.ci_release_key.current.as_ref().unwrap().algorithm,
        "ed25519"
    );
    assert!(cfg.ci_release_key.previous.is_none());
    assert!(cfg.ci_release_key.reject_before.is_none());
    assert!(cfg.attic_cache_key.is_none());
    assert!(cfg.org_root_key.is_none());
}

#[test]
fn key_slot_active_keys_returns_both_current_and_previous() {
    let slot = KeySlot {
        current: Some(TrustedPubkey {
            algorithm: "ed25519".into(),
            public: "AAAA".into(),
        }),
        previous: Some(TrustedPubkey {
            algorithm: "ecdsa-p256".into(),
            public: "BBBB".into(),
        }),
        reject_before: None,
    };
    let keys = slot.active_keys();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0].algorithm, "ed25519");
    assert_eq!(keys[1].algorithm, "ecdsa-p256");
}

#[test]
fn key_slot_active_keys_skips_absent() {
    let slot = KeySlot {
        current: None,
        previous: None,
        reject_before: None,
    };
    assert!(slot.active_keys().is_empty());
}

#[test]
fn attic_key_slot_accepts_native_format() {
    let json = r#""attic:cache.example.com:AAAA""#;
    let slot: AtticKeySlot = serde_json::from_str(json).unwrap();
    assert_eq!(slot.0, "attic:cache.example.com:AAAA");
}

#[test]
fn trust_config_rejects_missing_schema_version() {
    let json = r#"{
        "ciReleaseKey": { "current": null, "previous": null, "rejectBefore": null }
    }"#;
    let err = serde_json::from_str::<TrustConfig>(json).unwrap_err();
    assert!(err.to_string().contains("schemaVersion"), "got: {err}");
}
```

- [ ] **Step 3: Run tests to confirm they fail**

```bash
cargo test -p nixfleet-proto --test trust_config 2>&1 | tail -20
```

Expected: compile error on `TrustConfig`, `KeySlot`, `AtticKeySlot` — types don't exist.

- [ ] **Step 4: Implement `TrustConfig`, `KeySlot`, `AtticKeySlot`**

Append to `crates/nixfleet-proto/src/trust.rs`:

```rust
use chrono::{DateTime, Utc};

/// Trust configuration loaded from `/etc/nixfleet/{cp,agent}/trust.json`.
///
/// Shape authoritative per [`docs/trust-root-flow.md §3.4`][flow]. Materialised
/// by the NixOS scope modules from `config.nixfleet.trust`, consumed by CP
/// and agent binaries at startup.
///
/// Reload model: restart-only (§7.1). No SIGHUP, no inotify.
///
/// [flow]: ../../../docs/trust-root-flow.md
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustConfig {
    /// Required. Bumped only on breaking schema changes; binaries refuse
    /// to start on unknown versions (§7.4). The wire-protocol schema for
    /// `fleet.resolved` is separate (see `fleet_resolved::Meta`).
    pub schema_version: u32,

    pub ci_release_key: KeySlot,

    #[serde(default)]
    pub attic_cache_key: Option<AtticKeySlot>,

    #[serde(default)]
    pub org_root_key: Option<KeySlot>,
}

impl TrustConfig {
    /// The only `schemaVersion` value this crate parses. Binaries match on
    /// this after deserialisation and refuse unknown versions.
    pub const CURRENT_SCHEMA_VERSION: u32 = 1;
}

/// A single trust-root slot with current/previous rotation grace.
///
/// `reject_before` is the compromise switch — artifacts whose `signedAt`
/// is older than this timestamp are refused regardless of which key
/// signed them (§7.2). Enforcement lives in `verify_artifact`, not here.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeySlot {
    #[serde(default)]
    pub current: Option<TrustedPubkey>,

    #[serde(default)]
    pub previous: Option<TrustedPubkey>,

    #[serde(default)]
    pub reject_before: Option<DateTime<Utc>>,
}

impl KeySlot {
    /// Returns the active key list for this slot. Both `current` and
    /// `previous` are returned unconditionally when present.
    ///
    /// Signature per coordinator's context update: no `now` parameter;
    /// `reject_before` filtering happens inside `verify_artifact`.
    pub fn active_keys(&self) -> Vec<TrustedPubkey> {
        let mut keys = Vec::with_capacity(2);
        if let Some(k) = &self.current {
            keys.push(k.clone());
        }
        if let Some(k) = &self.previous {
            keys.push(k.clone());
        }
        keys
    }
}

/// Attic cache key in the attic-native string format `"attic:<host>:<base64>"`.
///
/// Typed as an opaque newtype because Stream B's `modules/_trust.nix`
/// currently keeps the attic key flat (CONTRACTS.md §II #2 has not yet
/// been migrated to the `{algorithm, public}` shape that §II #1 uses).
/// Migrates to `KeySlot<AtticPubkey>` when §II #2 gains that treatment.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(transparent)]
pub struct AtticKeySlot(pub String);
```

Update `crates/nixfleet-proto/src/lib.rs` re-exports:

```rust
pub use trust::{AtticKeySlot, KeySlot, TrustConfig, TrustedPubkey};
```

Update `crates/nixfleet-proto/Cargo.toml` to add `chrono` to `[dependencies]` if not already there (it likely already is transitively via `fleet_resolved`):

```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
chrono = { version = "0.4", features = ["serde"] }
```

- [ ] **Step 5: Run tests to confirm pass**

```bash
cargo test -p nixfleet-proto --test trust_config 2>&1 | tail -10
```

Expected: 5 tests pass.

Also run the full proto test suite:

```bash
cargo test -p nixfleet-proto 2>&1 | tail -10
```

Expected: all pre-existing proto tests + the unit-landmine test (Step 1a) + the 5 `TrustConfig` tests all pass.

- [ ] **Step 6: Commit**

```bash
git add crates/nixfleet-proto
git commit -m "feat(proto)(#29): TrustConfig + KeySlot + AtticKeySlot; Channel freshness_window MINUTES helper

TrustConfig shape per docs/trust-root-flow.md §3.4. schemaVersion is
required (§7.4) — TrustConfig::CURRENT_SCHEMA_VERSION is the only
version this crate parses; binaries gate on it at startup.

KeySlot::active_keys(&self) takes no 'now' and does not filter on
rejectBefore (coordinator's delta). rejectBefore enforcement lives
in verify_artifact (next commit).

Latent unit landmine: Channel.freshness_window is declared in MINUTES
by lib/mkFleet.nix but lacked a _minutes suffix in proto. Added a
doc-comment on the field and a Channel::freshness_window_duration()
helper so callers cannot accidentally pass the raw u32 into
Duration::from_secs. Test asserts 180 minutes → 10_800 seconds via
the helper; preserves the existing wire field name for compat."
```

### Task 1.6 — Extend `verify_artifact` with `reject_before`

**Files:** modify `crates/nixfleet-reconciler/src/verify.rs`; extend `crates/nixfleet-reconciler/tests/verify.rs`.

- [ ] **Step 1: Write the failing test for `RejectedBeforeTimestamp`**

Append to `crates/nixfleet-reconciler/tests/verify.rs`:

```rust
#[test]
fn rejects_artifact_older_than_reject_before() {
    let (canonical, signature, trust, signed_at) = sign_artifact(
        // reuse helper: ed25519-signed fixture with a known signed_at
        &fleet_resolved_fixture(),
    );
    let freshness = Duration::from_secs(86_400);
    let reject_before = signed_at + chrono::Duration::seconds(60);
    let now = signed_at + chrono::Duration::seconds(10);

    let err = verify_artifact(
        canonical.as_bytes(),
        &signature,
        std::slice::from_ref(&trust),
        now,
        freshness,
        Some(reject_before),
    )
    .unwrap_err();

    match err {
        VerifyError::RejectedBeforeTimestamp {
            signed_at: got_signed_at,
            reject_before: got_rb,
        } => {
            assert_eq!(got_signed_at, signed_at);
            assert_eq!(got_rb, reject_before);
        }
        other => panic!("expected RejectedBeforeTimestamp, got: {other:?}"),
    }
}

#[test]
fn accepts_artifact_signed_at_after_reject_before() {
    let (canonical, signature, trust, signed_at) = sign_artifact(&fleet_resolved_fixture());
    let freshness = Duration::from_secs(86_400);
    // reject_before older than the artifact — the artifact stays valid.
    let reject_before = signed_at - chrono::Duration::seconds(60);
    let now = signed_at + chrono::Duration::seconds(10);

    let fleet = verify_artifact(
        canonical.as_bytes(),
        &signature,
        std::slice::from_ref(&trust),
        now,
        freshness,
        Some(reject_before),
    )
    .expect("accepts artifact signed after rejectBefore");
    assert_eq!(fleet.schema_version, 1);
}

#[test]
fn reject_before_none_disables_the_gate() {
    let (canonical, signature, trust, _signed_at) = sign_artifact(&fleet_resolved_fixture());
    let freshness = Duration::from_secs(86_400);
    let now = chrono::Utc::now();

    let _fleet = verify_artifact(
        canonical.as_bytes(),
        &signature,
        std::slice::from_ref(&trust),
        now,
        freshness,
        None,
    )
    .expect("None means gate disabled");
}
```

Note: `fleet_resolved_fixture()` is a helper already in this file (used by existing tests). If the call site requires tweaks, adjust to match existing fixture shape. The key assertion is that `VerifyError::RejectedBeforeTimestamp { signed_at, reject_before }` is returned with the exact values.

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cargo test -p nixfleet-reconciler --test verify rejects_artifact_older_than_reject_before 2>&1 | tail -10
```

Expected: compile error — `verify_artifact` takes 5 args not 6; `RejectedBeforeTimestamp` variant doesn't exist.

- [ ] **Step 3: Add the `RejectedBeforeTimestamp` variant**

In `crates/nixfleet-reconciler/src/verify.rs`, add to the `VerifyError` enum (after `Stale`):

```rust
    #[error(
        "artifact signed at {signed_at} is older than reject_before {reject_before} (compromise switch, CONTRACTS.md §II #1)"
    )]
    RejectedBeforeTimestamp {
        signed_at: DateTime<Utc>,
        reject_before: DateTime<Utc>,
    },
```

- [ ] **Step 4: Add `reject_before` to `verify_artifact` signature**

Change `verify_artifact` signature to:

```rust
pub fn verify_artifact(
    signed_bytes: &[u8],
    signature: &[u8],
    trusted_keys: &[TrustedPubkey],
    now: DateTime<Utc>,
    freshness_window: Duration,
    reject_before: Option<DateTime<Utc>>,
) -> Result<FleetResolved, VerifyError> {
```

Thread `reject_before` through to `finish_verification`:

```rust
    // at the existing call sites of finish_verification, pass reject_before:
                    return finish_verification(&canonical, now, freshness_window, reject_before);
```

Update `finish_verification`:

```rust
fn finish_verification(
    canonical: &str,
    now: DateTime<Utc>,
    freshness_window: Duration,
    reject_before: Option<DateTime<Utc>>,
) -> Result<FleetResolved, VerifyError> {
    let fleet: FleetResolved = serde_json::from_str(canonical)?;

    if fleet.schema_version != ACCEPTED_SCHEMA_VERSION {
        return Err(VerifyError::SchemaVersionUnsupported(fleet.schema_version));
    }

    let signed_at = fleet.meta.signed_at.ok_or(VerifyError::NotSigned)?;

    // Slot-level compromise switch — applies to whichever key matched.
    if let Some(rb) = reject_before {
        if signed_at < rb {
            return Err(VerifyError::RejectedBeforeTimestamp {
                signed_at,
                reject_before: rb,
            });
        }
    }

    let window = ChronoDuration::from_std(freshness_window)
        .expect("freshness_window fits in i64 nanoseconds — multi-century windows are a bug");
    if now - signed_at > window {
        return Err(VerifyError::Stale {
            signed_at,
            now,
            window: freshness_window,
        });
    }

    Ok(fleet)
}
```

- [ ] **Step 5: Update the existing tests' `verify_artifact` call sites**

Existing tests in `crates/nixfleet-reconciler/tests/verify.rs` call `verify_artifact` with 5 args. Append `None` to every such call:

```bash
# Confirm the count of existing call sites needing an update:
grep -c "verify_artifact(" crates/nixfleet-reconciler/tests/verify.rs
```

Add `None,` as the last arg to each. Smart-edit each (sed risks mis-matching multi-line calls — use `Edit` per call site).

- [ ] **Step 6: Run all reconciler tests**

```bash
cargo test -p nixfleet-reconciler 2>&1 | tail -15
```

Expected: all tests pass (pre-existing + 3 new).

- [ ] **Step 7: Commit**

```bash
git add crates/nixfleet-reconciler
git commit -m "feat(reconciler)(#29): verify_artifact reject_before + RejectedBeforeTimestamp

Per docs/trust-root-flow.md §7.2 (decision locked in PR #25): the
rejectBefore compromise switch applies slot-wide — any artifact whose
signed_at is older than rejectBefore is refused regardless of which
key (current or previous) matched the signature.

The new VerifyError::RejectedBeforeTimestamp variant is distinct from
Stale — Stale means routine expiry against freshness_window, whereas
RejectedBeforeTimestamp means an operator-declared incident response.
Logs and alerts want to treat them differently."
```

### Task 1.7 — Scaffold empty v0.2 crates

**Files:** create `crates/nixfleet-agent/`, `crates/nixfleet-control-plane/`, `crates/nixfleet-cli/` trees.

Each scaffold is a compiling binary that prints a stub line and exits 0. PRs 2/3/4 replace the stub with functional code.

- [ ] **Step 1: Create `crates/nixfleet-agent/`**

```bash
mkdir -p crates/nixfleet-agent/src
```

Write `crates/nixfleet-agent/Cargo.toml`:

```toml
[package]
name = "nixfleet-agent"
version = "0.2.0"
edition = "2021"
description = "NixFleet fleet management agent (v0.2 poll-only skeleton)"
license = "MIT"
repository = "https://github.com/arcanesys/nixfleet"
homepage = "https://github.com/arcanesys/nixfleet"
authors = ["nixfleet contributors"]

[lib]
name = "nixfleet_agent"
path = "src/lib.rs"

[[bin]]
name = "nixfleet-agent"
path = "src/main.rs"

[dependencies]
nixfleet-proto = { path = "../nixfleet-proto" }
nixfleet-reconciler = { path = "../nixfleet-reconciler" }
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls-native-roots", "json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive", "env"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1"
thiserror = "2"

[dev-dependencies]
tempfile = "3"
wiremock = "0.6"
```

Write `crates/nixfleet-agent/src/lib.rs`:

```rust
//! NixFleet v0.2 agent library.
//!
//! The v0.2 agent is poll-only. It enrols via bootstrap token, fetches
//! an mTLS client certificate, checks in on cadence, reports
//! `currentGeneration`, and logs the target it *would* activate. It
//! never runs `nixos-rebuild switch`; activation is Phase 4 work gated
//! on Checkpoint 2.
//!
//! See `docs/KICKOFF.md §1 Stream C`, `rfcs/0003-protocol.md §4`, and
//! `docs/trust-root-flow.md §5`.
```

Write `crates/nixfleet-agent/src/main.rs`:

```rust
fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "nixfleet-agent v0.2 skeleton — functional body lands in PR 3"
    );
}
```

- [ ] **Step 2: Create `crates/nixfleet-control-plane/`**

```bash
mkdir -p crates/nixfleet-control-plane/src crates/nixfleet-control-plane/migrations
```

Write `crates/nixfleet-control-plane/Cargo.toml`:

```toml
[package]
name = "nixfleet-control-plane"
version = "0.2.0"
edition = "2021"
description = "NixFleet v0.2 control plane skeleton (Axum + SQLite + mTLS)"
license = "AGPL-3.0-only"
repository = "https://github.com/arcanesys/nixfleet"
homepage = "https://github.com/arcanesys/nixfleet"
authors = ["nixfleet contributors"]

[lib]
name = "nixfleet_control_plane"
path = "src/lib.rs"

[[bin]]
name = "nixfleet-control-plane"
path = "src/main.rs"

[dependencies]
nixfleet-proto = { path = "../nixfleet-proto" }
nixfleet-reconciler = { path = "../nixfleet-reconciler" }
axum = "0.8"
axum-server = { version = "0.7", features = ["tls-rustls"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive", "env"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
rusqlite = { version = "0.32", features = ["bundled"] }
refinery = { version = "0.8", features = ["rusqlite"] }
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1"
thiserror = "2"
rustls = "0.23"
tokio-rustls = "0.26"
x509-parser = "0.16"
tower-service = "0.3"
rustls-pki-types = "1"
http = "1"
sha2 = "0.10"
hex = "0.4"

[dev-dependencies]
tempfile = "3"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }
rcgen = "0.13"
tower = { version = "0.5", features = ["util"] }
```

Write `crates/nixfleet-control-plane/src/lib.rs`:

```rust
//! NixFleet v0.2 control plane library.
//!
//! v0.2 is an Axum + SQLite + mTLS skeleton serving the four wire
//! endpoints from RFC-0003 §4. It reads trust from a JSON file
//! (`docs/trust-root-flow.md §3.2`), polls a local `fleet.resolved.json`
//! path, calls `nixfleet_reconciler::verify_artifact` on every load,
//! refuses unverified artifacts, and logs reconcile actions per tick.
//!
//! Every SQLite column carries a `-- derivable from: …` comment per
//! `docs/CONTRACTS.md §IV` (storage purity rule).
```

Write `crates/nixfleet-control-plane/src/main.rs`:

```rust
fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "nixfleet-control-plane v0.2 skeleton — functional body lands in PR 2"
    );
}
```

Write a stub migration `crates/nixfleet-control-plane/migrations/V1__initial.sql`:

```sql
-- Placeholder migration. Real schema lands in PR 2.
-- Every column in this file carries a "-- derivable from:" comment
-- per docs/CONTRACTS.md §IV (storage purity rule).
CREATE TABLE schema_placeholder (
    id INTEGER PRIMARY KEY    -- derivable from: row ordinal, local only
);
```

- [ ] **Step 3: Create `crates/nixfleet-cli/`**

```bash
mkdir -p crates/nixfleet-cli/src
```

Write `crates/nixfleet-cli/Cargo.toml`:

```toml
[package]
name = "nixfleet-cli"
version = "0.2.0"
edition = "2021"
description = "NixFleet v0.2 operator CLI"
license = "MIT"
repository = "https://github.com/arcanesys/nixfleet"
homepage = "https://github.com/arcanesys/nixfleet"
authors = ["nixfleet contributors"]

[lib]
name = "nixfleet_cli"
path = "src/lib.rs"

[[bin]]
name = "nixfleet"
path = "src/main.rs"

[dependencies]
nixfleet-proto = { path = "../nixfleet-proto" }
clap = { version = "4", features = ["derive", "env"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
thiserror = "2"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
chrono = { version = "0.4", features = ["serde"] }
comfy-table = "7"

[dev-dependencies]
tempfile = "3"
assert_cmd = "2"
predicates = "3"
wiremock = "0.6"
```

Write `crates/nixfleet-cli/src/lib.rs`:

```rust
//! NixFleet v0.2 CLI library.
//!
//! v0.2 ships two subcommands — `status` and `rollout trace <id>` — the
//! minimum operator surface for Checkpoint 1 per `docs/KICKOFF.md §1
//! Stream C`. Later commands land incrementally as CP endpoints gain
//! operator-facing capabilities.
```

Write `crates/nixfleet-cli/src/main.rs`:

```rust
fn main() {
    tracing_subscriber::fmt::init();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "nixfleet CLI v0.2 skeleton — functional body lands in PR 4"
    );
}
```

- [ ] **Step 4: Verify the three binaries compile**

```bash
cargo check -p nixfleet-agent -p nixfleet-control-plane -p nixfleet-cli 2>&1 | tail -10
```

Expected: `Finished` with no errors. Dead-code warnings OK.

- [ ] **Step 5: Commit**

```bash
git add crates/nixfleet-agent crates/nixfleet-control-plane crates/nixfleet-cli
git commit -m "feat(#29): scaffold v0.2 agent, control-plane, cli crates

Empty binaries that print a stub line and exit 0. Functional bodies
land in PRs 2 (control-plane), 3 (agent), 4 (cli). The scaffold keeps
the workspace compiling while the migration completes."
```

### Task 1.8 — Rewrite `crane-workspace.nix`

**Files:** modify `crane-workspace.nix`.

- [ ] **Step 1: Update source filesets and per-crate entries**

Rewrite `crane-workspace.nix`:

```nix
# Crane-based workspace build — layered caching for independent packages,
# rebuild isolation, and shared dependency artifacts.
#
# Layers:
#   1. cargoArtifacts (buildDepsOnly) — shared compiled deps
#   2. Per-crate packages (buildPackage) — scoped source per crate, doCheck=false
#   3. workspace-tests (cargoTest) — one test run for the whole workspace
{
  lib,
  craneLib,
}: let
  workspaceSrc = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./crates
    ];
  };

  cargoArtifacts = craneLib.buildDepsOnly {
    src = workspaceSrc;
    pname = "nixfleet-workspace-deps";
  };

  fileSetForCrate = {
    crate,
    extraFiles ? [],
  }:
    lib.fileset.toSource {
      root = ./.;
      fileset = lib.fileset.unions ([
          ./Cargo.toml
          ./Cargo.lock
          (craneLib.fileset.commonCargoSources ./crates/nixfleet-proto)
          (craneLib.fileset.commonCargoSources ./crates/nixfleet-canonicalize)
          (craneLib.fileset.commonCargoSources ./crates/nixfleet-reconciler)
          (craneLib.fileset.commonCargoSources crate)
        ]
        ++ extraFiles);
    };

  commonArgs = {
    inherit cargoArtifacts;
    version = "0.2.0";
    doCheck = false;
  };

  nixfleet-agent = craneLib.buildPackage (commonArgs
    // {
      pname = "nixfleet-agent";
      cargoExtraArgs = "-p nixfleet-agent";
      src = fileSetForCrate {crate = ./crates/nixfleet-agent;};
      meta = {
        description = "NixFleet fleet management agent (v0.2 poll-only skeleton)";
        license = lib.licenses.mit;
        mainProgram = "nixfleet-agent";
      };
    });

  nixfleet-control-plane = craneLib.buildPackage (commonArgs
    // {
      pname = "nixfleet-control-plane";
      cargoExtraArgs = "-p nixfleet-control-plane";
      src = fileSetForCrate {
        crate = ./crates/nixfleet-control-plane;
        extraFiles = [./crates/nixfleet-control-plane/migrations];
      };
      meta = {
        description = "NixFleet v0.2 control plane skeleton";
        license = lib.licenses.agpl3Only;
        mainProgram = "nixfleet-control-plane";
      };
    });

  nixfleet-cli = craneLib.buildPackage (commonArgs
    // {
      pname = "nixfleet-cli";
      cargoExtraArgs = "-p nixfleet-cli";
      src = fileSetForCrate {crate = ./crates/nixfleet-cli;};
      meta = {
        description = "NixFleet v0.2 operator CLI";
        license = lib.licenses.mit;
        mainProgram = "nixfleet";
      };
    });

  nixfleet-canonicalize = craneLib.buildPackage (commonArgs
    // {
      pname = "nixfleet-canonicalize";
      cargoExtraArgs = "-p nixfleet-canonicalize";
      src = fileSetForCrate {crate = ./crates/nixfleet-canonicalize;};
      meta = {
        description = "JCS (RFC 8785) canonicalizer pinned per CONTRACTS.md §III";
        license = lib.licenses.mit;
        mainProgram = "nixfleet-canonicalize";
      };
    });

  workspace-tests = craneLib.cargoTest {
    inherit cargoArtifacts;
    src = workspaceSrc;
    pname = "nixfleet-workspace-tests";
    version = "0.2.0";
    cargoExtraArgs = "--workspace --locked";
  };
in {
  packages = {inherit nixfleet-agent nixfleet-control-plane nixfleet-cli nixfleet-canonicalize;};
  checks = {inherit workspace-tests;};
}
```

- [ ] **Step 2: Verify flake eval**

```bash
nix flake check --no-build --quiet 2>&1 | tail -15
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add crane-workspace.nix
git commit -m "build(crane)(#29): point workspace at v0.2 crate paths

Drops the crates/shared inclusion (nixfleet-types retired along with
v0.1). Per-crate fileset now shares nixfleet-proto, nixfleet-canonicalize,
and nixfleet-reconciler as the common v0.2 library surface.

Version bumped to 0.2.0 across the workspace."
```

### Task 1.9 — Rewrite the agent + CP scope modules

**Files:** modify `modules/scopes/nixfleet/_agent.nix`, `modules/scopes/nixfleet/_control-plane.nix`.

- [ ] **Step 1: Rewrite `modules/scopes/nixfleet/_agent.nix` in place**

Replace the entire file:

```nix
# NixOS service module for the NixFleet v0.2 agent (poll-only).
#
# Materialises /etc/nixfleet/agent/trust.json from config.nixfleet.trust
# per docs/trust-root-flow.md §3.2/§5, then launches the v0.2 agent
# binary with --trust-file pointing at the materialised file.
{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.nixfleet-agent;

  trustJson = pkgs.writers.writeJSON "agent-trust.json" {
    schemaVersion = 1;
    ciReleaseKey = config.nixfleet.trust.ciReleaseKey;
    atticCacheKey = config.nixfleet.trust.atticCacheKey.current;
    orgRootKey = config.nixfleet.trust.orgRootKey;
  };
in {
  options.services.nixfleet-agent = {
    enable = lib.mkEnableOption "NixFleet v0.2 fleet management agent";

    controlPlaneUrl = lib.mkOption {
      type = lib.types.str;
      example = "https://fleet.example.com";
      description = "URL of the NixFleet control plane.";
    };

    machineId = lib.mkOption {
      type = lib.types.str;
      default = config.hostSpec.hostName or config.networking.hostName;
      defaultText = lib.literalExpression "config.hostSpec.hostName";
      description = "Machine identifier reported to the control plane.";
    };

    pollInterval = lib.mkOption {
      type = lib.types.int;
      default = 60;
      description = "Poll interval in seconds (steady-state).";
    };

    trustFile = lib.mkOption {
      type = lib.types.path;
      default = "/etc/nixfleet/agent/trust.json";
      description = "Path to the trust-root JSON file (see docs/trust-root-flow.md §3.2).";
    };

    tls = {
      caCert = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/etc/nixfleet/fleet-ca.pem";
        description = "Path to CA certificate PEM file for verifying the control plane.";
      };
      clientCert = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/secrets/agent-cert.pem";
        description = "Path to client certificate PEM file for mTLS.";
      };
      clientKey = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/secrets/agent-key.pem";
        description = "Path to client private key PEM file for mTLS.";
      };
    };
  };

  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      environment.etc."nixfleet/agent/trust.json".source = trustJson;

      systemd.services.nixfleet-agent = {
        description = "NixFleet v0.2 Agent";
        wantedBy = ["multi-user.target"];
        after = ["network-online.target"];
        wants = ["network-online.target"];

        serviceConfig = {
          Type = "simple";
          ExecStart = lib.concatStringsSep " " (
            [
              "${pkgs.nixfleet-agent}/bin/nixfleet-agent"
              "--control-plane-url"
              (lib.escapeShellArg cfg.controlPlaneUrl)
              "--machine-id"
              (lib.escapeShellArg cfg.machineId)
              "--poll-interval"
              (toString cfg.pollInterval)
              "--trust-file"
              (lib.escapeShellArg cfg.trustFile)
            ]
            ++ lib.optionals (cfg.tls.caCert != null) ["--ca-cert" (lib.escapeShellArg cfg.tls.caCert)]
            ++ lib.optionals (cfg.tls.clientCert != null) ["--client-cert" (lib.escapeShellArg cfg.tls.clientCert)]
            ++ lib.optionals (cfg.tls.clientKey != null) ["--client-key" (lib.escapeShellArg cfg.tls.clientKey)]
          );
          Restart = "always";
          RestartSec = 30;
          StateDirectory = "nixfleet";
          NoNewPrivileges = true;
        };
      };
    })

    (lib.mkIf (cfg.enable && (config.nixfleet.impermanence.enable or false)) {
      environment.persistence."/persist".directories = ["/var/lib/nixfleet"];
    })
  ];
}
```

Note the `pkgs.nixfleet-agent` call — the package must be injected via `nixpkgs.overlays` or via a `disabledModules` + import pattern. Check the existing repo pattern — if `modules/rust-packages.nix` or a nearby file already overlays the workspace packages into `pkgs`, no further change needed. Otherwise add an overlay to the scope module:

```nix
  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      nixpkgs.overlays = lib.mkBefore [
        (_final: _prev: {
          inherit (inputs.self.packages.${pkgs.system}) nixfleet-agent;
        })
      ];
      # … rest of config …
    })
  ];
```

The import head will need `inputs` back in the args — compare against `_agent.nix` before deletion for the existing pattern. If the current pattern was `pkgs.callPackage ../../../crates/agent {inherit inputs;}`, replicate at the new path: `pkgs.callPackage ../../../crates/nixfleet-agent {inherit inputs;}`. That's simpler than the overlay; use it.

Adjust accordingly — the final ExecStart reference becomes `${nixfleet-agent}/bin/nixfleet-agent` where `nixfleet-agent = pkgs.callPackage ../../../crates/nixfleet-agent {inherit inputs;};` in the `let` binding at the top of the file.

- [ ] **Step 2: Rewrite `modules/scopes/nixfleet/_control-plane.nix` in place**

Same pattern, adding `--release-path`:

```nix
{
  config,
  inputs,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.nixfleet-control-plane;
  nixfleet-control-plane = pkgs.callPackage ../../../crates/nixfleet-control-plane {inherit inputs;};

  trustJson = pkgs.writers.writeJSON "cp-trust.json" {
    schemaVersion = 1;
    ciReleaseKey = config.nixfleet.trust.ciReleaseKey;
    atticCacheKey = config.nixfleet.trust.atticCacheKey.current;
    orgRootKey = config.nixfleet.trust.orgRootKey;
  };
in {
  options.services.nixfleet-control-plane = {
    enable = lib.mkEnableOption "NixFleet v0.2 control plane server";

    listen = lib.mkOption {
      type = lib.types.str;
      default = "0.0.0.0:8080";
      description = "Address and port to listen on.";
    };

    dbPath = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/nixfleet-cp/state.db";
      description = "Path to the SQLite state database.";
    };

    trustFile = lib.mkOption {
      type = lib.types.path;
      default = "/etc/nixfleet/cp/trust.json";
      description = "Path to the trust-root JSON file (docs/trust-root-flow.md §3.2).";
    };

    releasePath = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/nixfleet-cp/fleet.git/releases/fleet.resolved.json";
      description = "Path to the signed fleet.resolved.json artifact the CP polls.";
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Open the control plane port in the firewall.";
    };

    tls = {
      cert = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Path to TLS certificate PEM file.";
      };
      key = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Path to TLS private key PEM file.";
      };
      clientCa = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Path to client CA PEM file for mTLS agent authentication.";
      };
    };
  };

  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      assertions = [
        {
          assertion = builtins.match ".*:[0-9]+" cfg.listen != null;
          message = ''services.nixfleet-control-plane.listen must be HOST:PORT, got: "${cfg.listen}"'';
        }
      ];

      environment.etc."nixfleet/cp/trust.json".source = trustJson;

      systemd.services.nixfleet-control-plane = {
        description = "NixFleet v0.2 Control Plane";
        wantedBy = ["multi-user.target"];
        after = ["network-online.target"];
        wants = ["network-online.target"];

        serviceConfig = {
          Type = "simple";
          ExecStart = lib.concatStringsSep " " (
            [
              "${nixfleet-control-plane}/bin/nixfleet-control-plane"
              "--listen"
              (lib.escapeShellArg cfg.listen)
              "--db-path"
              (lib.escapeShellArg cfg.dbPath)
              "--trust-file"
              (lib.escapeShellArg cfg.trustFile)
              "--release-path"
              (lib.escapeShellArg cfg.releasePath)
            ]
            ++ lib.optionals (cfg.tls.cert != null) ["--tls-cert" (lib.escapeShellArg cfg.tls.cert)]
            ++ lib.optionals (cfg.tls.key != null) ["--tls-key" (lib.escapeShellArg cfg.tls.key)]
            ++ lib.optionals (cfg.tls.clientCa != null) ["--client-ca" (lib.escapeShellArg cfg.tls.clientCa)]
          );
          Restart = "always";
          RestartSec = 10;
          StateDirectory = "nixfleet-cp";

          NoNewPrivileges = true;
          ProtectHome = true;
          PrivateTmp = true;
          PrivateDevices = true;
          ProtectKernelTunables = true;
          ProtectKernelModules = true;
          ProtectControlGroups = true;
          ReadWritePaths = ["/var/lib/nixfleet-cp"];
        };
      };

      networking.firewall.allowedTCPPorts = let
        port = lib.toInt (lib.last (lib.splitString ":" cfg.listen));
      in
        lib.mkIf cfg.openFirewall [port];
    })

    (lib.mkIf (cfg.enable && (config.nixfleet.impermanence.enable or false)) {
      environment.persistence."/persist".directories = ["/var/lib/nixfleet-cp"];
    })
  ];
}
```

- [ ] **Step 3: Write eval tests for the trust-file wiring**

Create `modules/tests/_agent-v2-trust.nix`:

```nix
# Eval-only test: agent module materialises /etc/nixfleet/agent/trust.json.
{
  self,
  lib,
  pkgs,
  ...
}: let
  sys = lib.nixosSystem {
    inherit (pkgs.stdenv.hostPlatform) system;
    modules = [
      self.nixosModules.nixfleet
      {
        nixpkgs.hostPlatform = pkgs.stdenv.hostPlatform.system;
        networking.hostName = "agent-trust-test";
        services.nixfleet-agent = {
          enable = true;
          controlPlaneUrl = "https://cp.test:8080";
        };
        nixfleet.trust.ciReleaseKey.current = {
          algorithm = "ed25519";
          public = "AAAA";
        };
        boot.loader.grub.enable = false;
        fileSystems."/" = {
          device = "/dev/null";
          fsType = "ext4";
        };
      }
    ];
  };
  evaluated = sys.config.environment.etc."nixfleet/agent/trust.json".source;
in {
  etcEntryMaterialises = builtins.pathExists evaluated;
  execStartCarriesTrustFile =
    lib.any
    (arg: arg == "--trust-file")
    sys.config.systemd.services.nixfleet-agent.serviceConfig.ExecStart;
  execStartCarriesControlPlaneUrl =
    lib.any (arg: arg == "--control-plane-url") sys.config.systemd.services.nixfleet-agent.serviceConfig.ExecStart;
}
```

Similarly for CP: `modules/tests/_cp-v2-trust.nix` with asserts on `--trust-file`, `--release-path`, `--db-path`.

Wire both into `modules/tests/eval.nix` under an existing `eval-*` attribute or a new `eval-nixfleet-trust-wiring`. The existing file already has a similar pattern — match it.

- [ ] **Step 4: Run eval tests**

```bash
nix flake check --no-build --quiet 2>&1 | tail -20
```

Expected: green. New eval tests appear in the output list.

- [ ] **Step 5: Commit**

```bash
git add modules/scopes/nixfleet/_agent.nix modules/scopes/nixfleet/_control-plane.nix modules/tests/
git commit -m "feat(modules)(#29): v0.2 agent + CP scope modules write trust.json

Materialise config.nixfleet.trust into /etc/nixfleet/{agent,cp}/trust.json
via environment.etc and pass --trust-file on ExecStart. CP also gets
--release-path (default /var/lib/nixfleet-cp/fleet.git/releases/
fleet.resolved.json — per docs/trust-root-flow.md §4 option b).

Restart-only reload per §7.1: nixos-rebuild switch changes the etc
entry content, systemd restarts the service, binary re-reads trust.

Eval tests in modules/tests/ verify the wiring produces the expected
ExecStart shape and etc entry."
```

### Task 1.10 — Rebase safety check: workspace compiles, eval green

**Files:** none.

- [ ] **Step 1: Final workspace check**

```bash
cargo check --workspace 2>&1 | tail -5
```

Expected: `Finished` across every crate.

- [ ] **Step 2: Final flake eval**

```bash
nix flake check --no-build --quiet 2>&1 | tail -15
```

Expected: no errors. VM tests disappeared along with `_vm-fleet-scenarios/`.

- [ ] **Step 3: Push PR 1 branch and open PR**

```bash
git push -u origin feat/29-m1p3-migration
```

Present the branch to the user; do not open the PR without explicit "ship it" confirmation per workflow-preferences.

### PR 1 acceptance checklist

- [ ] `cargo check --workspace` clean.
- [ ] `nix flake check --no-build` clean.
- [ ] No references to the deleted v0.1 crates anywhere in `modules/`, `flake.nix`, `crane-workspace.nix`.
- [ ] `nixfleet-proto::TrustConfig` + `KeySlot` + `AtticKeySlot` exist with round-trip tests.
- [ ] `nixfleet-reconciler::verify_artifact` takes `reject_before: Option<DateTime<Utc>>` + tests for `RejectedBeforeTimestamp`.
- [ ] v0.2 crate binaries compile and print a stub line.
- [ ] Scope modules write trust.json via `environment.etc` and pass `--trust-file` (+ `--release-path` for CP) on ExecStart.
- [ ] PR body lists the reviewer full-gauntlet commands (see "Hand off" section below).

---

## PR 2 — Control plane skeleton

Branch `feat/29-m1p3-cp` off `feat/29-m1p3-migration`. Title: `feat(cp)(#29): v0.2 control plane — Axum + SQLite + mTLS + 4 endpoints + verify_artifact on load`.

### Overview

The CP is the biggest of the three skeletons. Responsibilities:

1. Parse CLI args (`--listen`, `--db-path`, `--tls-cert`, `--tls-key`, `--client-ca`, `--trust-file`, `--release-path`).
2. Load `TrustConfig` from `--trust-file`; refuse to start if `schemaVersion != 1` or the file is missing/unparsable.
3. Open SQLite; run refinery migrations; apply PRAGMAs (`foreign_keys = ON`, `journal_mode = WAL`).
4. Start a release-path poller: every tick (default 15s), read `<release-path>` and `<release-path>.sig`, call `verify_artifact(…)`, refuse on failure (log and keep the last verified artifact); on success, store the verified artifact in a CP state struct.
5. Start a reconcile-tick task: every tick (default 30s), call `nixfleet_reconciler::reconcile(…)` with the last-verified `FleetResolved` + observed state from SQLite + rollout history. Log the returned `Vec<Action>`; stub persistence.
6. Start the Axum server bound to `<listen>` with TLS (if configured) + mTLS client-ca validation. Four routes:
   - `POST /v1/agent/checkin` — accepts `CheckinRequest`, updates observed state, returns `CheckinResponse` with optional `target`.
   - `POST /v1/agent/confirm` — accepts `ConfirmRequest`, closes the confirm window, returns `204` or `410`.
   - `POST /v1/agent/report` — accepts `ReportRequest`, logs + stores. Returns `202`.
   - `GET /v1/agent/closure/<hash>` — stub. Returns `404 Not Found` + body `closure proxy not implemented in v0.2 skeleton`.
7. Middleware: on every route, enforce `cert.CN == request.body.hostname` per RFC-0003 §7. Use `x509-parser` to extract CN from the peer cert.

### Task 2.1 — Create PR 2 branch

- [ ] **Step 1: Branch**

```bash
git checkout -b feat/29-m1p3-cp feat/29-m1p3-migration
```

### Task 2.2 — Add wire protocol types to `nixfleet-proto`

**Files:** create `crates/nixfleet-proto/src/wire.rs`; modify `crates/nixfleet-proto/src/lib.rs`; create `crates/nixfleet-proto/tests/wire.rs`.

- [ ] **Step 1: Write the failing round-trip test**

Create `crates/nixfleet-proto/tests/wire.rs`:

```rust
use nixfleet_proto::wire::{
    CheckinRequest, CheckinResponse, ConfirmRequest, CurrentGeneration, Health, ReportRequest,
    Target, TargetActivate, TargetRuntimeProbe,
};

#[test]
fn checkin_request_roundtrips_rfc_0003_41_example() {
    let json = r#"{
        "hostname": "m70q-attic",
        "agentVersion": "0.2.1",
        "currentGeneration": {
            "closureHash": "sha256-aabbcc",
            "channelRef": "abc123def",
            "bootId": "f0e1d2c3-0000-0000-0000-000000000000"
        },
        "health": {
            "systemdFailedUnits": [],
            "uptime": 1234567,
            "loadAverage": [0.1, 0.2, 0.3]
        },
        "lastProbeResults": []
    }"#;
    let req: CheckinRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.hostname, "m70q-attic");
    assert_eq!(req.current_generation.closure_hash, "sha256-aabbcc");
    let round = serde_json::to_string(&req).unwrap();
    let re: CheckinRequest = serde_json::from_str(&round).unwrap();
    assert_eq!(re.hostname, req.hostname);
}

#[test]
fn checkin_response_with_target_roundtrips() {
    let json = r#"{
        "target": {
            "closureHash": "sha256-ddeeff",
            "channelRef": "def456abc",
            "rollout": "stable@def456",
            "wave": 2,
            "activate": {
                "confirmWindowSecs": 120,
                "confirmEndpoint": "/v1/agent/confirm",
                "runtimeProbes": [
                    { "control": "anssi-bp028-ssh-no-password", "schema": "anssi-bp028/v1" }
                ]
            }
        },
        "nextCheckinSecs": 60
    }"#;
    let resp: CheckinResponse = serde_json::from_str(json).unwrap();
    let t = resp.target.expect("has target");
    assert_eq!(t.wave, 2);
    assert_eq!(t.activate.confirm_window_secs, 120);
}

#[test]
fn checkin_response_without_target_roundtrips() {
    let json = r#"{ "nextCheckinSecs": 300 }"#;
    let resp: CheckinResponse = serde_json::from_str(json).unwrap();
    assert!(resp.target.is_none());
    assert_eq!(resp.next_checkin_secs, 300);
}

#[test]
fn confirm_request_roundtrips() {
    let json = r#"{
        "hostname": "m70q-attic",
        "rollout": "stable@def456",
        "wave": 2,
        "generation": {
            "closureHash": "sha256-ddeeff",
            "bootId": "11111111-1111-1111-1111-111111111111"
        },
        "probeResults": []
    }"#;
    let req: ConfirmRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.wave, 2);
    assert_eq!(req.generation.closure_hash, "sha256-ddeeff");
}

#[test]
fn report_request_activation_failed_roundtrips() {
    let json = r#"{
        "hostname": "m70q-attic",
        "event": "activation-failed",
        "rollout": "stable@def456",
        "details": {
            "phase": "switch-to-configuration",
            "exitCode": 1,
            "stderrTail": "..."
        }
    }"#;
    let req: ReportRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.event, "activation-failed");
}
```

- [ ] **Step 2: Run tests — expect compile failure**

```bash
cargo test -p nixfleet-proto --test wire 2>&1 | tail -10
```

- [ ] **Step 3: Implement wire types**

Create `crates/nixfleet-proto/src/wire.rs`:

```rust
//! Wire protocol v1 types — agent ↔ control plane (RFC-0003 §4).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinRequest {
    pub hostname: String,
    pub agent_version: String,
    pub current_generation: CurrentGeneration,
    pub health: Health,
    #[serde(default)]
    pub last_probe_results: Vec<ProbeResult>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentGeneration {
    pub closure_hash: String,
    pub channel_ref: String,
    pub boot_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    #[serde(default)]
    pub systemd_failed_units: Vec<String>,
    pub uptime: u64,
    pub load_average: [f64; 3],
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub control: String,
    pub status: String,
    #[serde(default)]
    pub evidence: Option<String>,
    pub ts: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinResponse {
    #[serde(default)]
    pub target: Option<Target>,
    pub next_checkin_secs: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub closure_hash: String,
    pub channel_ref: String,
    pub rollout: String,
    pub wave: u32,
    pub activate: TargetActivate,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetActivate {
    pub confirm_window_secs: u64,
    pub confirm_endpoint: String,
    #[serde(default)]
    pub runtime_probes: Vec<TargetRuntimeProbe>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetRuntimeProbe {
    pub control: String,
    pub schema: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmRequest {
    pub hostname: String,
    pub rollout: String,
    pub wave: u32,
    pub generation: ConfirmGeneration,
    #[serde(default)]
    pub probe_results: Vec<ProbeResult>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmGeneration {
    pub closure_hash: String,
    pub boot_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportRequest {
    pub hostname: String,
    pub event: String,
    #[serde(default)]
    pub rollout: Option<String>,
    #[serde(default)]
    pub details: serde_json::Value,
}
```

Extend `crates/nixfleet-proto/src/lib.rs`:

```rust
pub mod wire;
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p nixfleet-proto 2>&1 | tail -10
```

Expected: all proto tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/nixfleet-proto
git commit -m "feat(proto)(#29): wire protocol v1 types (RFC-0003 §4)

CheckinRequest/Response, ConfirmRequest, ReportRequest, Target and
sub-types. camelCase serde rename, absent optional fields round-trip
as null (consistent with existing proto posture)."
```

### Task 2.3 — CP CLI + TrustConfig load + SQLite + migrations

**Files:** create `crates/nixfleet-control-plane/src/cli.rs`, `src/state.rs`, `src/db.rs`; rewrite `src/main.rs`; replace `migrations/V1__initial.sql`.

- [ ] **Step 1: Write the failing integration test**

Create `crates/nixfleet-control-plane/tests/cli.rs`:

```rust
use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

const SAMPLE_TRUST_JSON: &str = r#"{
    "schemaVersion": 1,
    "ciReleaseKey": {
        "current": { "algorithm": "ed25519", "public": "AAAA" },
        "previous": null,
        "rejectBefore": null
    },
    "atticCacheKey": null,
    "orgRootKey": null
}"#;

#[test]
fn refuses_missing_trust_file() {
    let td = TempDir::new().unwrap();
    let mut cmd = Command::cargo_bin("nixfleet-control-plane").unwrap();
    cmd.args([
        "--listen",
        "127.0.0.1:0",
        "--db-path",
        td.path().join("cp.db").to_str().unwrap(),
        "--trust-file",
        "/nonexistent/trust.json",
        "--release-path",
        td.path().join("fleet.resolved.json").to_str().unwrap(),
        "--print-startup-info-and-exit",
    ]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("trust file"));
}

#[test]
fn refuses_wrong_schema_version() {
    let td = TempDir::new().unwrap();
    let trust = td.path().join("trust.json");
    std::fs::write(
        &trust,
        SAMPLE_TRUST_JSON.replace("\"schemaVersion\": 1", "\"schemaVersion\": 99"),
    )
    .unwrap();
    let mut cmd = Command::cargo_bin("nixfleet-control-plane").unwrap();
    cmd.args([
        "--listen",
        "127.0.0.1:0",
        "--db-path",
        td.path().join("cp.db").to_str().unwrap(),
        "--trust-file",
        trust.to_str().unwrap(),
        "--release-path",
        td.path().join("fleet.resolved.json").to_str().unwrap(),
        "--print-startup-info-and-exit",
    ]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("schemaVersion"));
}

#[test]
fn startup_info_prints_and_exits_cleanly() {
    let td = TempDir::new().unwrap();
    let trust = td.path().join("trust.json");
    std::fs::write(&trust, SAMPLE_TRUST_JSON).unwrap();
    let mut cmd = Command::cargo_bin("nixfleet-control-plane").unwrap();
    cmd.args([
        "--listen",
        "127.0.0.1:0",
        "--db-path",
        td.path().join("cp.db").to_str().unwrap(),
        "--trust-file",
        trust.to_str().unwrap(),
        "--release-path",
        td.path().join("fleet.resolved.json").to_str().unwrap(),
        "--print-startup-info-and-exit",
    ]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("trust_file_loaded"));
}
```

Add `assert_cmd = "2"` + `predicates = "3"` to dev-deps if not already present (they are — see PR 1's Cargo.toml).

- [ ] **Step 2: Run test — expect binary-level failure (stub main doesn't honour flags)**

```bash
cargo test -p nixfleet-control-plane --test cli 2>&1 | tail -10
```

- [ ] **Step 3: Implement `cli.rs`**

Create `crates/nixfleet-control-plane/src/cli.rs`:

```rust
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(version, about = "NixFleet v0.2 control plane")]
pub struct Cli {
    /// Listen address (HOST:PORT).
    #[arg(long)]
    pub listen: String,

    /// Path to the SQLite state database.
    #[arg(long)]
    pub db_path: PathBuf,

    /// Path to TLS certificate PEM file (enables HTTPS when set with --tls-key).
    #[arg(long)]
    pub tls_cert: Option<PathBuf>,

    /// Path to TLS private key PEM file.
    #[arg(long)]
    pub tls_key: Option<PathBuf>,

    /// Path to client CA PEM file (enables mTLS when set).
    #[arg(long)]
    pub client_ca: Option<PathBuf>,

    /// Path to the trust-root JSON file (docs/trust-root-flow.md §3.2).
    #[arg(long)]
    pub trust_file: PathBuf,

    /// Path to the signed fleet.resolved.json artifact to poll.
    #[arg(long)]
    pub release_path: PathBuf,

    /// Freshness window in seconds when verifying fleet.resolved (default 86400 = 24h).
    #[arg(long, default_value_t = 86_400)]
    pub freshness_window_secs: u64,

    /// Reconcile tick interval in seconds.
    #[arg(long, default_value_t = 30)]
    pub reconcile_tick_secs: u64,

    /// Release poll interval in seconds.
    #[arg(long, default_value_t = 15)]
    pub release_poll_secs: u64,

    /// Print startup-validated configuration to stdout and exit. Used by
    /// integration tests to verify CLI + config loading without binding
    /// a socket or spawning the server.
    #[arg(long)]
    pub print_startup_info_and_exit: bool,
}
```

- [ ] **Step 4: Implement `state.rs`**

Create `crates/nixfleet-control-plane/src/state.rs`:

```rust
use anyhow::{anyhow, Context, Result};
use nixfleet_proto::{FleetResolved, TrustConfig};
use std::path::Path;
use std::sync::{Arc, RwLock};

#[derive(Clone)]
pub struct AppState {
    pub trust: Arc<TrustConfig>,
    pub last_verified_artifact: Arc<RwLock<Option<FleetResolved>>>,
}

/// Loads TrustConfig from disk, validates schemaVersion == 1.
pub fn load_trust_file(path: &Path) -> Result<TrustConfig> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("trust file {} unreadable", path.display()))?;
    let cfg: TrustConfig =
        serde_json::from_str(&text).with_context(|| format!("trust file {} unparsable", path.display()))?;
    if cfg.schema_version != TrustConfig::CURRENT_SCHEMA_VERSION {
        return Err(anyhow!(
            "trust file {}: unsupported schemaVersion={} (expected {})",
            path.display(),
            cfg.schema_version,
            TrustConfig::CURRENT_SCHEMA_VERSION
        ));
    }
    Ok(cfg)
}
```

- [ ] **Step 5: Implement `db.rs` + migration**

Create `crates/nixfleet-control-plane/src/db.rs`:

```rust
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

refinery::embed_migrations!("migrations");

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db dir {}", parent.display()))?;
        }
    }
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrations::runner().run(&mut conn).context("refinery migrations")?;
    Ok(conn)
}
```

Replace `crates/nixfleet-control-plane/migrations/V1__initial.sql`:

```sql
-- NixFleet v0.2 control plane — initial schema.
-- Every column carries a `-- derivable from:` comment per docs/CONTRACTS.md §IV.
--
-- Skeleton-level schema: just enough to persist observed check-in state.
-- Fuller tables (rollouts, waves, events) land in later PRs.

CREATE TABLE hosts (
    hostname        TEXT PRIMARY KEY,   -- derivable from: fleet.resolved (Stream B)
    current_gen_hash TEXT,              -- derivable from: agent check-in
    current_channel_ref TEXT,           -- derivable from: agent check-in
    current_boot_id TEXT,               -- derivable from: agent check-in
    last_seen_at    DATETIME NOT NULL   -- derivable from: agent check-in
);

CREATE TABLE rollout_events (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,  -- derivable from: row ordinal; discarded on teardown per CONTRACTS.md §IV accepted-loss
    ts              DATETIME NOT NULL,                  -- derivable from: event emission time, discarded on teardown
    rollout         TEXT NOT NULL,                      -- derivable from: reconcile tick output
    wave            INTEGER,                            -- derivable from: reconcile tick output
    hostname        TEXT,                               -- derivable from: reconcile tick output
    transition      TEXT NOT NULL,                      -- derivable from: reconcile tick output
    reason          TEXT NOT NULL                       -- derivable from: reconcile tick output
);
```

- [ ] **Step 6: Rewrite `src/main.rs`**

```rust
use anyhow::Result;
use clap::Parser;
use nixfleet_control_plane::cli::Cli;
use nixfleet_control_plane::{db, state};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,axum=info".into()),
        )
        .init();

    let cli = Cli::parse();

    let trust = state::load_trust_file(&cli.trust_file).inspect_err(|e| {
        tracing::error!(error = %e, "trust file load failed");
    })?;

    let _conn = db::open(&cli.db_path).inspect_err(|e| {
        tracing::error!(error = %e, "db open failed");
    })?;

    if cli.print_startup_info_and_exit {
        println!(
            "trust_file_loaded listen={} db_path={} ci_release_algorithm={:?}",
            cli.listen,
            cli.db_path.display(),
            trust.ci_release_key.current.as_ref().map(|k| &k.algorithm)
        );
        return Ok(());
    }

    tracing::info!(listen = %cli.listen, "nixfleet-control-plane startup OK — server wiring lands in the next task");
    Ok(())
}
```

Update `src/lib.rs`:

```rust
pub mod cli;
pub mod db;
pub mod state;
```

- [ ] **Step 7: Run tests**

```bash
cargo test -p nixfleet-control-plane --test cli 2>&1 | tail -10
```

Expected: 3 tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/nixfleet-control-plane
git commit -m "feat(cp)(#29): CLI + TrustConfig load + SQLite + migrations

--trust-file / --release-path / --db-path wiring. TrustConfig loader
refuses schemaVersion != 1 with a clear error. Initial SQLite schema
has hosts + rollout_events tables, every column annotated with
'-- derivable from:' per CONTRACTS.md §IV."
```

### Task 2.4 — HTTP routes (checkin / confirm / report / closure stub)

**Files:** create `src/routes/mod.rs`, `src/routes/checkin.rs`, `src/routes/confirm.rs`, `src/routes/report.rs`, `src/routes/closure.rs`; tests at `tests/routes.rs`.

- [ ] **Step 1: Write the failing route test**

Create `crates/nixfleet-control-plane/tests/routes.rs`:

```rust
//! Route-level integration tests — tower::oneshot without a TLS socket.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nixfleet_control_plane::routes;
use nixfleet_control_plane::state::AppState;
use nixfleet_proto::{KeySlot, TrustConfig, TrustedPubkey};
use std::sync::{Arc, RwLock};
use tower::ServiceExt;

fn mock_state() -> AppState {
    AppState {
        trust: Arc::new(TrustConfig {
            schema_version: 1,
            ci_release_key: KeySlot {
                current: Some(TrustedPubkey {
                    algorithm: "ed25519".into(),
                    public: "AAAA".into(),
                }),
                previous: None,
                reject_before: None,
            },
            attic_cache_key: None,
            org_root_key: None,
        }),
        last_verified_artifact: Arc::new(RwLock::new(None)),
    }
}

#[tokio::test]
async fn checkin_accepts_valid_body() {
    let app = routes::router(mock_state());
    let body = serde_json::to_vec(&serde_json::json!({
        "hostname": "h1",
        "agentVersion": "0.2.0",
        "currentGeneration": { "closureHash": "sha256-aa", "channelRef": "r1", "bootId": "b1" },
        "health": { "systemdFailedUnits": [], "uptime": 1, "loadAverage": [0.0, 0.0, 0.0] },
        "lastProbeResults": []
    }))
    .unwrap();
    let req = Request::post("/v1/agent/checkin")
        .header("content-type", "application/json")
        .header("X-Nixfleet-Protocol", "1")
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn checkin_rejects_wrong_protocol_version() {
    let app = routes::router(mock_state());
    let req = Request::post("/v1/agent/checkin")
        .header("X-Nixfleet-Protocol", "2")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn closure_endpoint_returns_501_stub() {
    let app = routes::router(mock_state());
    let req = Request::get("/v1/agent/closure/sha256-aa")
        .header("X-Nixfleet-Protocol", "1")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn report_accepts_and_returns_202() {
    let app = routes::router(mock_state());
    let body = serde_json::to_vec(&serde_json::json!({
        "hostname": "h1",
        "event": "activation-failed",
        "rollout": "stable@abc"
    }))
    .unwrap();
    let req = Request::post("/v1/agent/report")
        .header("content-type", "application/json")
        .header("X-Nixfleet-Protocol", "1")
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
}
```

- [ ] **Step 2: Run test — expect compile failure**

- [ ] **Step 3: Implement `routes/mod.rs`**

```rust
use crate::state::AppState;
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};

pub mod checkin;
pub mod closure;
pub mod confirm;
pub mod report;

const WIRE_VERSION: &str = "1";

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/agent/checkin", post(checkin::handler))
        .route("/v1/agent/confirm", post(confirm::handler))
        .route("/v1/agent/report", post(report::handler))
        .route("/v1/agent/closure/:hash", get(closure::handler))
        .layer(middleware::from_fn(protocol_header_guard))
        .with_state(state)
}

async fn protocol_header_guard(
    headers: HeaderMap,
    req: axum::extract::Request,
    next: Next,
) -> Result<Response, Response> {
    match headers.get("X-Nixfleet-Protocol").and_then(|v| v.to_str().ok()) {
        Some(v) if v == WIRE_VERSION => Ok(next.run(req).await),
        Some(v) => Err((
            StatusCode::BAD_REQUEST,
            format!("unsupported X-Nixfleet-Protocol: {v} (expected {WIRE_VERSION})"),
        )
            .into_response()),
        None => Err((
            StatusCode::BAD_REQUEST,
            "missing X-Nixfleet-Protocol header".to_string(),
        )
            .into_response()),
    }
}
```

Create `src/routes/checkin.rs`:

```rust
use crate::state::AppState;
use axum::{extract::State, http::StatusCode, Json};
use nixfleet_proto::wire::{CheckinRequest, CheckinResponse};

pub async fn handler(
    State(_state): State<AppState>,
    Json(req): Json<CheckinRequest>,
) -> Result<Json<CheckinResponse>, StatusCode> {
    tracing::info!(hostname = %req.hostname, gen = %req.current_generation.closure_hash, "checkin");
    // Skeleton: no observed-state persistence, no target computation.
    // Respond with the idle-poll default cadence.
    Ok(Json(CheckinResponse {
        target: None,
        next_checkin_secs: 60,
    }))
}
```

Create `src/routes/confirm.rs`:

```rust
use crate::state::AppState;
use axum::{extract::State, http::StatusCode, Json};
use nixfleet_proto::wire::ConfirmRequest;

pub async fn handler(
    State(_state): State<AppState>,
    Json(req): Json<ConfirmRequest>,
) -> StatusCode {
    tracing::info!(hostname = %req.hostname, rollout = %req.rollout, wave = req.wave, "confirm");
    StatusCode::NO_CONTENT
}
```

Create `src/routes/report.rs`:

```rust
use crate::state::AppState;
use axum::{extract::State, http::StatusCode, Json};
use nixfleet_proto::wire::ReportRequest;

pub async fn handler(
    State(_state): State<AppState>,
    Json(req): Json<ReportRequest>,
) -> StatusCode {
    tracing::warn!(hostname = %req.hostname, event = %req.event, details = ?req.details, "agent report");
    StatusCode::ACCEPTED
}
```

Create `src/routes/closure.rs`:

```rust
use axum::{extract::Path, http::StatusCode};

pub async fn handler(Path(hash): Path<String>) -> (StatusCode, &'static str) {
    tracing::debug!(closure = %hash, "closure proxy stub — v0.2 skeleton does not implement this");
    (
        StatusCode::NOT_IMPLEMENTED,
        "closure proxy not implemented in v0.2 skeleton",
    )
}
```

Update `src/lib.rs`:

```rust
pub mod cli;
pub mod db;
pub mod routes;
pub mod state;
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p nixfleet-control-plane --test routes 2>&1 | tail -10
```

Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/nixfleet-control-plane
git commit -m "feat(cp)(#29): four wire endpoints per RFC-0003 §4

checkin / confirm / report / closure. X-Nixfleet-Protocol=1 enforced
by middleware — mismatched version returns 400. closure endpoint
returns 501 with a stub message; v0.2 does not proxy closures.

Route handlers are thin skeletons: they log + return the happy path
response shape. Observed-state persistence and target computation
land when the reconcile tick is wired to the SQLite tables."
```

### Task 2.5 — mTLS bind + CN-vs-hostname middleware

**Files:** create `src/tls.rs`, `src/auth_cn.rs`; rewrite the server-startup portion of `src/main.rs`.

See RFC-0003 §7 — every route enforces `cert.CN == request.body.hostname`. Use `x509-parser` to extract the subject CN from the peer cert supplied by `tokio-rustls`.

- [ ] **Step 1: Write the failing mTLS CN-mismatch test**

Create `crates/nixfleet-control-plane/tests/mtls_cn.rs`:

```rust
//! mTLS CN-vs-hostname enforcement (RFC-0003 §7).
//!
//! Uses rcgen to synthesise a self-signed cert with a known CN, feeds
//! that cert into a tower::oneshot pipeline that sets the CN extension
//! the same way the production wrapper will. Does NOT spin up a real
//! TLS socket — that coverage lives in VM tests later.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use nixfleet_control_plane::auth_cn::{cn_enforcement_layer, ClientCn};
use nixfleet_control_plane::routes;
use nixfleet_control_plane::state::AppState;
use nixfleet_proto::{KeySlot, TrustConfig, TrustedPubkey};
use std::sync::{Arc, RwLock};
use tower::ServiceExt;

fn mock_state() -> AppState {
    AppState {
        trust: Arc::new(TrustConfig {
            schema_version: 1,
            ci_release_key: KeySlot {
                current: Some(TrustedPubkey {
                    algorithm: "ed25519".into(),
                    public: "AAAA".into(),
                }),
                previous: None,
                reject_before: None,
            },
            attic_cache_key: None,
            org_root_key: None,
        }),
        last_verified_artifact: Arc::new(RwLock::new(None)),
    }
}

async fn oneshot_with_cn(cn: &str, body: serde_json::Value) -> StatusCode {
    let app = routes::router(mock_state()).layer(cn_enforcement_layer());
    let mut req = Request::post("/v1/agent/checkin")
        .header("content-type", "application/json")
        .header("X-Nixfleet-Protocol", "1")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    req.extensions_mut().insert(ClientCn(cn.to_string()));
    app.oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn accepts_matching_cn() {
    let status = oneshot_with_cn(
        "agent-01",
        serde_json::json!({
            "hostname": "agent-01",
            "agentVersion": "0.2.0",
            "currentGeneration": { "closureHash": "sha256-aa", "channelRef": "r", "bootId": "b" },
            "health": { "systemdFailedUnits": [], "uptime": 1, "loadAverage": [0.0, 0.0, 0.0] },
            "lastProbeResults": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn rejects_cn_mismatch_with_forbidden() {
    let status = oneshot_with_cn(
        "agent-01",
        serde_json::json!({
            "hostname": "other-host",
            "agentVersion": "0.2.0",
            "currentGeneration": { "closureHash": "sha256-aa", "channelRef": "r", "bootId": "b" },
            "health": { "systemdFailedUnits": [], "uptime": 1, "loadAverage": [0.0, 0.0, 0.0] },
            "lastProbeResults": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
```

- [ ] **Step 2: Implement `src/auth_cn.rs`**

```rust
//! Client CN extraction from mTLS peer cert + CN-vs-hostname middleware.
//!
//! The production wrapper around tokio-rustls parses the peer cert via
//! x509-parser and inserts a `ClientCn` extension into the request. The
//! layer below pulls `ClientCn` out of `request.extensions()` and
//! enforces equality with the body's `hostname` field.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    middleware::{from_fn, Next},
    response::{IntoResponse, Response},
};
use tower::ServiceBuilder;

#[derive(Clone, Debug)]
pub struct ClientCn(pub String);

pub fn cn_enforcement_layer() -> tower::layer::util::Stack<
    axum::middleware::FromFnLayer<fn(Request<Body>, Next) -> _, (), _>,
    tower::layer::util::Identity,
> {
    // Simpler typed signature: return a layer the caller can apply directly.
    // In practice we use `.layer(from_fn(enforce_cn))` at the call site.
    ServiceBuilder::new().layer(from_fn(enforce_cn))
}

pub async fn enforce_cn(mut req: Request<Body>, next: Next) -> Result<Response, Response> {
    let cn = match req.extensions().get::<ClientCn>() {
        Some(cn) => cn.0.clone(),
        None => {
            return Err((
                StatusCode::UNAUTHORIZED,
                "no client certificate CN available",
            )
                .into_response())
        }
    };

    // Peek at JSON body's `hostname` field without consuming the request.
    let (parts, body) = req.into_parts();
    let bytes = to_bytes(body, 64 * 1024).await.map_err(|e| {
        (StatusCode::BAD_REQUEST, format!("body read: {e}")).into_response()
    })?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    let body_hostname = value.get("hostname").and_then(|v| v.as_str()).unwrap_or("");

    if body_hostname != cn {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "CN/hostname mismatch: cert CN='{cn}', body hostname='{body_hostname}'"
            ),
        )
            .into_response());
    }

    // Rebuild request with the captured body.
    req = Request::from_parts(parts, Body::from(bytes));
    Ok(next.run(req).await)
}
```

Note: the above middleware consumes the body to peek at `hostname`, then reconstructs the body. Acceptable for a skeleton because requests are tiny JSON; 64 KiB cap prevents DoS. A real production wrapper would use a typed extractor. Keep simple.

Adjust `cn_enforcement_layer` — the explicit type signature above is gnarly; use a `Layer` alias or inline `.layer(from_fn(enforce_cn))` at the call site and remove the `cn_enforcement_layer` helper if it proves fragile. Test the `.layer(from_fn(enforce_cn))` pattern directly.

Revise the test's layer construction to:

```rust
let app = routes::router(mock_state()).layer(axum::middleware::from_fn(enforce_cn));
```

- [ ] **Step 3: Implement `src/tls.rs` — peer-cert CN extractor**

```rust
//! mTLS peer-cert CN extraction, wired into axum-server acceptor.

use anyhow::{anyhow, Result};
use axum::body::Body;
use axum::http::Request;
use rustls::server::{ServerConfig, WebPkiClientVerifier};
use rustls::RootCertStore;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::path::Path;
use std::sync::Arc;
use x509_parser::prelude::*;

use crate::auth_cn::ClientCn;

pub fn load_server_config(
    cert_path: &Path,
    key_path: &Path,
    client_ca_path: &Path,
) -> Result<ServerConfig> {
    let certs = load_certs(cert_path)?;
    let key = load_key(key_path)?;
    let ca = load_certs(client_ca_path)?;
    let mut roots = RootCertStore::empty();
    for c in ca {
        roots.add(c)?;
    }
    let verifier = WebPkiClientVerifier::builder(roots.into()).build()?;
    let cfg = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)?;
    Ok(cfg)
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let mut r = std::io::BufReader::new(std::fs::File::open(path)?);
    Ok(rustls_pemfile::certs(&mut r)
        .collect::<Result<Vec<_>, _>>()?)
}

fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let mut r = std::io::BufReader::new(std::fs::File::open(path)?);
    rustls_pemfile::private_key(&mut r)?.ok_or_else(|| anyhow!("no private key in {}", path.display()))
}

/// Extract the subject CN from the peer certificate and inject it into
/// the request as `ClientCn`. Called by the axum-server / tokio-rustls
/// wrapper after the handshake completes.
pub fn inject_cn_from_peer_cert<B>(
    peer_cert_der: &CertificateDer<'_>,
    req: &mut Request<B>,
) -> Result<()> {
    let (_, parsed) = X509Certificate::from_der(peer_cert_der.as_ref())
        .map_err(|e| anyhow!("x509 parse: {e}"))?;
    let cn = parsed
        .subject()
        .iter_common_name()
        .next()
        .and_then(|attr| attr.as_str().ok())
        .ok_or_else(|| anyhow!("peer cert has no CN"))?;
    req.extensions_mut().insert(ClientCn(cn.to_string()));
    Ok(())
}
```

Add to `Cargo.toml`:

```toml
rustls-pemfile = "2"
```

- [ ] **Step 4: Wire mTLS server startup in `src/main.rs` — guarded behind `--tls-cert/--tls-key/--client-ca` triple**

Append to `main.rs` (after `print_startup_info_and_exit`):

```rust
    use nixfleet_control_plane::{auth_cn::enforce_cn, routes, state::AppState};
    use std::sync::{Arc, RwLock};

    let app_state = AppState {
        trust: Arc::new(trust),
        last_verified_artifact: Arc::new(RwLock::new(None)),
    };

    let app = routes::router(app_state).layer(axum::middleware::from_fn(enforce_cn));

    let addr: std::net::SocketAddr = cli.listen.parse()?;

    match (cli.tls_cert.as_ref(), cli.tls_key.as_ref(), cli.client_ca.as_ref()) {
        (Some(cert), Some(key), Some(ca)) => {
            let cfg = nixfleet_control_plane::tls::load_server_config(cert, key, ca)?;
            let tls = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(cfg));
            tracing::info!(addr = %addr, "nixfleet-control-plane listening (mTLS)");
            axum_server::bind_rustls(addr, tls)
                .serve(app.into_make_service())
                .await?;
        }
        _ => {
            tracing::info!(addr = %addr, "nixfleet-control-plane listening (plain HTTP — development only)");
            axum_server::bind(addr).serve(app.into_make_service()).await?;
        }
    }
    Ok(())
```

Note: when mTLS is bound, `ClientCn` injection happens inside the `axum-server::Rustls` acceptor wrapper. axum-server's `rustls::RustlsConfig` doesn't expose peer-cert hooks as cleanly as raw `tokio_rustls::TlsAcceptor` — if the accessor path is fragile, consider a custom `Acceptor` impl at `src/tls.rs` that wraps `TlsAcceptor::accept` and sets the `ClientCn` via `rustls::ServerConnection::peer_certificates()`. The v0.1 `crates/control-plane/src/auth_cn.rs` (now deleted) had a working version that used `tokio_rustls::TlsStream` + `ServerConnection::peer_certificates()` — consult git history for reference if needed:

```bash
git show pr-27:crates/control-plane/src/auth_cn.rs 2>&1 | head -100
```

(Note: `pr-27` is the harness; v0.1 auth_cn lives in the commit before our trim. Fetch via `git log --all --oneline -- crates/control-plane/src/auth_cn.rs` to find the last commit touching it.)

- [ ] **Step 5: Run tests**

```bash
cargo test -p nixfleet-control-plane 2>&1 | tail -20
```

Expected: all CP tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/nixfleet-control-plane
git commit -m "feat(cp)(#29): mTLS bind + CN-vs-hostname enforcement (RFC-0003 §7)

Server binds TLS with client-ca verification. After handshake, the
peer cert's subject CN is injected into the request as ClientCn; a
middleware layer enforces request.body.hostname == ClientCn — mismatch
returns 403 Forbidden.

tower::oneshot-based tests cover the happy path and the mismatch;
real-TLS-socket coverage lives in VM tests (Phase 2)."
```

### Task 2.6 — Release-path poller + verify on load + reconcile tick

**Files:** create `src/release.rs`, `src/reconcile.rs`; wire tokio tasks in `src/main.rs`.

- [ ] **Step 1: Write the failing release-verify test**

Create `crates/nixfleet-control-plane/tests/release.rs`:

```rust
//! Integration test — CP refuses an unsigned/tampered fleet.resolved.

use nixfleet_control_plane::release::load_and_verify;
use nixfleet_proto::{KeySlot, TrustConfig, TrustedPubkey};
use tempfile::TempDir;
use std::time::Duration;

fn trust_with(algorithm: &str, public: &str) -> TrustConfig {
    TrustConfig {
        schema_version: 1,
        ci_release_key: KeySlot {
            current: Some(TrustedPubkey {
                algorithm: algorithm.into(),
                public: public.into(),
            }),
            previous: None,
            reject_before: None,
        },
        attic_cache_key: None,
        org_root_key: None,
    }
}

#[test]
fn refuses_missing_release_file() {
    let td = TempDir::new().unwrap();
    let cfg = trust_with("ed25519", "AAAA");
    let err = load_and_verify(
        &td.path().join("fleet.resolved.json"),
        &cfg,
        chrono::Utc::now(),
        Duration::from_secs(86_400),
    )
    .unwrap_err();
    assert!(err.to_string().contains("unreadable") || err.to_string().contains("No such file"));
}

#[test]
fn refuses_missing_signature_file() {
    let td = TempDir::new().unwrap();
    let resolved = td.path().join("fleet.resolved.json");
    std::fs::write(&resolved, r#"{ "schemaVersion": 1 }"#).unwrap();
    let cfg = trust_with("ed25519", "AAAA");
    let err = load_and_verify(&resolved, &cfg, chrono::Utc::now(), Duration::from_secs(86_400))
        .unwrap_err();
    assert!(err.to_string().contains("signature") || err.to_string().contains(".sig"));
}
```

- [ ] **Step 2: Implement `src/release.rs`**

```rust
//! Release-path poller: reads <release-path> + <release-path>.sig, calls
//! verify_artifact, returns the verified FleetResolved. Any failure
//! leaves the caller's state unchanged (fail closed).

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use nixfleet_proto::{FleetResolved, TrustConfig};
use nixfleet_reconciler::verify_artifact;
use std::path::Path;
use std::time::Duration;

pub fn load_and_verify(
    release_path: &Path,
    trust: &TrustConfig,
    now: DateTime<Utc>,
    freshness_window: Duration,
) -> Result<FleetResolved> {
    let bytes = std::fs::read(release_path)
        .with_context(|| format!("release file {} unreadable", release_path.display()))?;
    let sig_path = release_path.with_extension("json.sig");
    let signature = std::fs::read(&sig_path)
        .with_context(|| format!("signature file {} unreadable", sig_path.display()))?;

    let ci_keys = trust.ci_release_key.active_keys();
    if ci_keys.is_empty() {
        return Err(anyhow!(
            "no active ciReleaseKey — trust.json has neither current nor previous set"
        ));
    }
    let reject_before = trust.ci_release_key.reject_before;

    verify_artifact(&bytes, &signature, &ci_keys, now, freshness_window, reject_before)
        .map_err(|e| anyhow!("verify_artifact: {e}"))
}
```

- [ ] **Step 3: Run test**

```bash
cargo test -p nixfleet-control-plane --test release 2>&1 | tail -10
```

Expected: 2 tests pass.

- [ ] **Step 4: Wire the release poller + reconcile tick in `src/main.rs`**

Replace the server-startup block with:

```rust
    // Spawn release poller + reconcile tick.
    let state_for_release = app_state.clone();
    let release_path = cli.release_path.clone();
    let trust_for_release = app_state.trust.clone();
    let freshness = Duration::from_secs(cli.freshness_window_secs);
    let release_interval = Duration::from_secs(cli.release_poll_secs);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(release_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            match nixfleet_control_plane::release::load_and_verify(
                &release_path,
                &trust_for_release,
                chrono::Utc::now(),
                freshness,
            ) {
                Ok(fleet) => {
                    let mut guard = state_for_release.last_verified_artifact.write().unwrap();
                    *guard = Some(fleet);
                    tracing::info!("release loaded + verified");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "release load/verify failed — keeping last verified artifact");
                }
            }
        }
    });

    let state_for_reconcile = app_state.clone();
    let reconcile_interval = Duration::from_secs(cli.reconcile_tick_secs);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(reconcile_interval);
        loop {
            ticker.tick().await;
            let guard = state_for_reconcile.last_verified_artifact.read().unwrap();
            match guard.as_ref() {
                Some(_fleet) => {
                    // Skeleton: call reconciler with minimum observed state.
                    // Full reconcile wiring lands in a follow-up PR once rollout
                    // tables exist in SQLite; here we just log the tick.
                    tracing::debug!("reconcile tick (skeleton, no actions)");
                }
                None => tracing::debug!("reconcile tick skipped — no verified artifact yet"),
            }
        }
    });
```

- [ ] **Step 5: Run full CP test suite**

```bash
cargo test -p nixfleet-control-plane 2>&1 | tail -15
```

- [ ] **Step 6: Commit**

```bash
git add crates/nixfleet-control-plane
git commit -m "feat(cp)(#29): release-path poller + verify_artifact on load + reconcile tick

Release poller reads <release-path> + <release-path>.sig every
<release-poll-secs>. Calls verify_artifact with the active
ciReleaseKey slice + freshness_window + reject_before. On success,
stores the verified FleetResolved in AppState. On failure, logs and
keeps the previous verified artifact — fail closed.

Reconcile tick runs every <reconcile-tick-secs>. Skeleton logs the
tick; full Vec<Action> persistence lands in a later PR with rollout
tables."
```

### PR 2 acceptance checklist

- [ ] `cargo test -p nixfleet-control-plane` green (target: 15+ new tests).
- [ ] CP refuses missing/malformed trust file, mismatched schemaVersion.
- [ ] Four wire endpoints respond with correct status + body shape.
- [ ] mTLS binding wired with CN-vs-hostname enforcement.
- [ ] Release poller + reconcile tick logged on a live instance (verify manually via `cargo run --bin nixfleet-control-plane -- …`).
- [ ] SQLite migration applies; every column has `-- derivable from:`.

---

## PR 3 — Agent skeleton

Branch `feat/29-m1p3-agent` off `feat/29-m1p3-cp`. Title: `feat(agent)(#29): v0.2 agent skeleton — poll + enroll + checkin + direct-fetch verify`.

### Task 3.1 — Create PR 3 branch + add agent CLI

**Files:** create `crates/nixfleet-agent/src/cli.rs`; rewrite `src/main.rs`; create `tests/cli.rs`.

- [ ] **Step 1: Branch**

```bash
git checkout -b feat/29-m1p3-agent feat/29-m1p3-cp
```

- [ ] **Step 2: Write failing CLI test**

Create `crates/nixfleet-agent/tests/cli.rs`:

```rust
use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

const SAMPLE_TRUST_JSON: &str = r#"{
    "schemaVersion": 1,
    "ciReleaseKey": { "current": { "algorithm": "ed25519", "public": "AAAA" }, "previous": null, "rejectBefore": null },
    "atticCacheKey": null,
    "orgRootKey": null
}"#;

#[test]
fn refuses_missing_trust_file() {
    let mut cmd = Command::cargo_bin("nixfleet-agent").unwrap();
    cmd.args([
        "--control-plane-url", "https://cp.test",
        "--machine-id", "h1",
        "--trust-file", "/nonexistent/trust.json",
        "--poll-interval", "60",
        "--print-startup-info-and-exit",
    ]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("trust file"));
}

#[test]
fn accepts_valid_trust_file_and_exits() {
    let td = TempDir::new().unwrap();
    let trust = td.path().join("trust.json");
    std::fs::write(&trust, SAMPLE_TRUST_JSON).unwrap();
    let mut cmd = Command::cargo_bin("nixfleet-agent").unwrap();
    cmd.args([
        "--control-plane-url", "https://cp.test",
        "--machine-id", "h1",
        "--trust-file", trust.to_str().unwrap(),
        "--poll-interval", "60",
        "--print-startup-info-and-exit",
    ]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("startup_ok"));
}
```

- [ ] **Step 3: Implement `src/cli.rs`**

```rust
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(version, about = "NixFleet v0.2 agent (poll-only skeleton)")]
pub struct Cli {
    #[arg(long)]
    pub control_plane_url: String,

    #[arg(long)]
    pub machine_id: String,

    #[arg(long, default_value_t = 60)]
    pub poll_interval: u64,

    #[arg(long)]
    pub trust_file: PathBuf,

    #[arg(long)]
    pub ca_cert: Option<PathBuf>,

    #[arg(long)]
    pub client_cert: Option<PathBuf>,

    #[arg(long)]
    pub client_key: Option<PathBuf>,

    #[arg(long, default_value_t = 86_400)]
    pub freshness_window_secs: u64,

    #[arg(long)]
    pub print_startup_info_and_exit: bool,
}
```

- [ ] **Step 4: Rewrite `src/main.rs`**

```rust
use anyhow::{Context, Result};
use clap::Parser;
use nixfleet_agent::cli::Cli;
use nixfleet_proto::TrustConfig;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    let trust_text = std::fs::read_to_string(&cli.trust_file)
        .with_context(|| format!("trust file {} unreadable", cli.trust_file.display()))?;
    let trust: TrustConfig = serde_json::from_str(&trust_text)
        .with_context(|| format!("trust file {} unparsable", cli.trust_file.display()))?;
    if trust.schema_version != TrustConfig::CURRENT_SCHEMA_VERSION {
        anyhow::bail!(
            "trust file {}: unsupported schemaVersion={} (expected {})",
            cli.trust_file.display(),
            trust.schema_version,
            TrustConfig::CURRENT_SCHEMA_VERSION
        );
    }

    if cli.print_startup_info_and_exit {
        println!(
            "startup_ok cp={} machine_id={} ci_alg={:?}",
            cli.control_plane_url,
            cli.machine_id,
            trust.ci_release_key.current.as_ref().map(|k| &k.algorithm)
        );
        return Ok(());
    }

    tracing::info!(cp = %cli.control_plane_url, machine = %cli.machine_id, "nixfleet-agent v0.2 starting");
    nixfleet_agent::run::run(cli, trust).await
}
```

Update `src/lib.rs`:

```rust
pub mod cli;
pub mod config;
pub mod enroll;
pub mod checkin;
pub mod fetch;
pub mod run;
pub mod tls;
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p nixfleet-agent --test cli 2>&1 | tail -10
```

- [ ] **Step 6: Commit**

```bash
git add crates/nixfleet-agent
git commit -m "feat(agent)(#29): CLI + TrustConfig gate + lib-module scaffold"
```

### Task 3.2 — Checkin loop

**Files:** create `crates/nixfleet-agent/src/checkin.rs`, `src/run.rs`; add wiremock-based integration test at `tests/checkin.rs`.

- [ ] **Step 1: Write the failing test against a wiremock CP**

```rust
//! Wiremock-based checkin test — full HTTP roundtrip against a stub CP.

use nixfleet_agent::checkin::{build_client, checkin_once};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn checkin_once_sends_expected_body_and_parses_response() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/agent/checkin"))
        .and(header("X-Nixfleet-Protocol", "1"))
        .and(header("Content-Type", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "target": null,
            "nextCheckinSecs": 30
        })))
        .mount(&server)
        .await;

    let client = build_client(None, None, None).unwrap();
    let resp = checkin_once(
        &client,
        &server.uri(),
        "agent-01",
        "0.2.0",
        nixfleet_proto::wire::CurrentGeneration {
            closure_hash: "sha256-aa".into(),
            channel_ref: "r".into(),
            boot_id: "b".into(),
        },
    )
    .await
    .unwrap();

    assert_eq!(resp.next_checkin_secs, 30);
    assert!(resp.target.is_none());
}
```

- [ ] **Step 2: Implement `src/checkin.rs`**

```rust
//! Agent check-in: one HTTP request to POST /v1/agent/checkin.

use anyhow::Result;
use nixfleet_proto::wire::{CheckinRequest, CheckinResponse, CurrentGeneration, Health};
use reqwest::Client;
use std::path::Path;

pub fn build_client(
    ca_cert: Option<&Path>,
    client_cert: Option<&Path>,
    client_key: Option<&Path>,
) -> Result<Client> {
    let mut builder = Client::builder().use_rustls_tls();
    if let Some(ca) = ca_cert {
        let bytes = std::fs::read(ca)?;
        let cert = reqwest::Certificate::from_pem(&bytes)?;
        builder = builder.add_root_certificate(cert);
    }
    if let (Some(c), Some(k)) = (client_cert, client_key) {
        let mut pem = std::fs::read(c)?;
        pem.extend_from_slice(&std::fs::read(k)?);
        let identity = reqwest::Identity::from_pem(&pem)?;
        builder = builder.identity(identity);
    }
    Ok(builder.build()?)
}

pub async fn checkin_once(
    client: &Client,
    cp_url: &str,
    hostname: &str,
    agent_version: &str,
    current_generation: CurrentGeneration,
) -> Result<CheckinResponse> {
    let body = CheckinRequest {
        hostname: hostname.into(),
        agent_version: agent_version.into(),
        current_generation,
        health: Health {
            systemd_failed_units: vec![],
            uptime: 0,
            load_average: [0.0, 0.0, 0.0],
        },
        last_probe_results: vec![],
    };
    let resp = client
        .post(format!("{cp_url}/v1/agent/checkin"))
        .header("X-Nixfleet-Protocol", "1")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?
        .error_for_status()?;
    Ok(resp.json().await?)
}
```

- [ ] **Step 3: Implement `src/run.rs`**

```rust
//! Agent main loop — periodic check-in at `cli.poll_interval` cadence.

use anyhow::Result;
use nixfleet_proto::{wire::CurrentGeneration, TrustConfig};
use std::time::Duration;

use crate::checkin::{build_client, checkin_once};
use crate::cli::Cli;

pub async fn run(cli: Cli, _trust: TrustConfig) -> Result<()> {
    let client = build_client(
        cli.ca_cert.as_deref(),
        cli.client_cert.as_deref(),
        cli.client_key.as_deref(),
    )?;

    let mut ticker = tokio::time::interval(Duration::from_secs(cli.poll_interval));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        let current = CurrentGeneration {
            closure_hash: read_current_closure_hash().unwrap_or_else(|_| "unknown".into()),
            channel_ref: "unknown".into(),
            boot_id: read_boot_id().unwrap_or_else(|_| "unknown".into()),
        };
        match checkin_once(
            &client,
            &cli.control_plane_url,
            &cli.machine_id,
            env!("CARGO_PKG_VERSION"),
            current,
        )
        .await
        {
            Ok(resp) => {
                if let Some(target) = resp.target.as_ref() {
                    tracing::info!(
                        rollout = %target.rollout,
                        wave = target.wave,
                        closure = %target.closure_hash,
                        "target received (v0.2 skeleton: not activating)"
                    );
                } else {
                    tracing::debug!(next_secs = resp.next_checkin_secs, "no target, idle");
                }
            }
            Err(e) => tracing::warn!(error = %e, "checkin failed; will retry at next interval"),
        }
    }
}

fn read_current_closure_hash() -> Result<String> {
    // Minimal: read /run/current-system and hash it. Skeleton — real impl
    // reads /run/current-system/nix-support/system or uses nix-store -q.
    // The v0.1 code in crates/agent/src/nix.rs (now deleted) shows a
    // production implementation if needed.
    Ok(std::fs::read_link("/run/current-system")?
        .to_string_lossy()
        .into())
}

fn read_boot_id() -> Result<String> {
    Ok(std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .into())
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p nixfleet-agent 2>&1 | tail -15
```

- [ ] **Step 5: Commit**

```bash
git add crates/nixfleet-agent
git commit -m "feat(agent)(#29): periodic checkin loop against CP

build_client supports optional CA + client-cert/key for mTLS.
checkin_once issues POST /v1/agent/checkin with wire v1 header and
parses CheckinResponse. run() ticks at --poll-interval, logs target
receipt, never activates."
```

### Task 3.3 — Direct-fetch fallback path with `verify_artifact`

**Files:** create `crates/nixfleet-agent/src/fetch.rs`; tests at `tests/fetch.rs`.

Scope: agent has an optional fallback that fetches `fleet.resolved.json` + `.sig` directly (e.g. from Forgejo raw HTTP or from CP's `/v1/fleet/release` if added later), verifies via `verify_artifact`, and uses the active attic-cache trust root when deciding to pull closures. For the skeleton, we expose a callable function + integration test — no production code path wires it yet (that lives in a later PR once the direct-fetch trigger criterion is pinned).

- [ ] **Step 1: Write the failing wiremock test for `fetch_and_verify`**

```rust
use nixfleet_agent::fetch::fetch_and_verify;
use nixfleet_proto::{KeySlot, TrustConfig, TrustedPubkey};
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn trust_ed25519(public: &str) -> TrustConfig {
    TrustConfig {
        schema_version: 1,
        ci_release_key: KeySlot {
            current: Some(TrustedPubkey { algorithm: "ed25519".into(), public: public.into() }),
            previous: None,
            reject_before: None,
        },
        attic_cache_key: None,
        org_root_key: None,
    }
}

#[tokio::test]
async fn fetch_and_verify_rejects_bad_signature() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/fleet.resolved.json"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"schemaVersion":1}"#))
        .mount(&server).await;
    Mock::given(method("GET")).and(path("/fleet.resolved.json.sig"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 64]))
        .mount(&server).await;

    let err = fetch_and_verify(
        &format!("{}/fleet.resolved.json", server.uri()),
        &trust_ed25519("AAAA"),
        chrono::Utc::now(),
        Duration::from_secs(86_400),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("verify"));
}
```

- [ ] **Step 2: Implement `src/fetch.rs`**

```rust
//! Direct-fetch fallback: GET <url> + <url>.sig, verify via reconciler.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use nixfleet_proto::{FleetResolved, TrustConfig};
use nixfleet_reconciler::verify_artifact;
use std::time::Duration;

pub async fn fetch_and_verify(
    url: &str,
    trust: &TrustConfig,
    now: DateTime<Utc>,
    freshness_window: Duration,
) -> Result<FleetResolved> {
    let client = reqwest::Client::builder().use_rustls_tls().build()?;
    let bytes = client.get(url).send().await?.error_for_status()?.bytes().await?;
    let sig_url = format!("{url}.sig");
    let sig = client.get(&sig_url).send().await?.error_for_status()?.bytes().await?;

    let ci_keys = trust.ci_release_key.active_keys();
    let reject_before = trust.ci_release_key.reject_before;
    verify_artifact(&bytes, &sig, &ci_keys, now, freshness_window, reject_before)
        .map_err(|e| anyhow!("verify failed: {e}"))
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p nixfleet-agent 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```bash
git add crates/nixfleet-agent
git commit -m "feat(agent)(#29): direct-fetch fallback + verify_artifact

Callable fetch_and_verify — GET <url> + <url>.sig, call verify_artifact
with the active ciReleaseKey slice. No production call site yet; the
v0.2 agent keeps relying on CP's check-in responses for target info
until a later PR adds the fallback trigger."
```

### PR 3 acceptance checklist

- [ ] `cargo test -p nixfleet-agent` green (target: 10+ tests).
- [ ] Agent refuses malformed / mismatched-schemaVersion trust file.
- [ ] `checkin_once` issues expected body + header; parses response.
- [ ] `fetch_and_verify` rejects bad signatures with a clear error.
- [ ] Poll loop logs target receipt without activating.

---

## PR 4 — CLI skeleton

Branch `feat/29-m1p3-cli` off `feat/29-m1p3-agent`. Title: `feat(cli)(#29): nixfleet status + rollout trace`.

### Task 4.1 — Create PR 4 branch + `nixfleet status`

**Files:** create `crates/nixfleet-cli/src/cli.rs`, `src/client.rs`, `src/status.rs`; rewrite `src/main.rs`; tests.

For `status`, the CP needs a new read-only endpoint that returns the fleet overview. Add a minimal `GET /v1/fleet/status` returning `{ hosts: [{ hostname, currentGenerationHash, lastSeenAt }] }`. Extend PR 2's work or add it in this PR — add here to keep PR 2's review surface tighter.

- [ ] **Step 1: Branch**

```bash
git checkout -b feat/29-m1p3-cli feat/29-m1p3-agent
```

- [ ] **Step 2: Add `GET /v1/fleet/status` route in CP**

Create `crates/nixfleet-control-plane/src/routes/status.rs`:

```rust
use crate::state::AppState;
use axum::{extract::State, Json};
use serde::Serialize;

#[derive(Serialize)]
pub struct FleetStatus {
    pub hosts: Vec<HostStatus>,
    pub has_verified_artifact: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    pub hostname: String,
    pub current_generation_hash: Option<String>,
    pub last_seen_at: Option<String>,
}

pub async fn handler(State(state): State<AppState>) -> Json<FleetStatus> {
    let has = state.last_verified_artifact.read().unwrap().is_some();
    // Skeleton: empty host list. Full version queries SQLite.
    Json(FleetStatus { hosts: vec![], has_verified_artifact: has })
}
```

Register in `src/routes/mod.rs`:

```rust
pub mod status;
// …
    .route("/v1/fleet/status", get(status::handler))
```

- [ ] **Step 3: Implement CLI**

`src/cli.rs`:

```rust
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(version, about = "NixFleet v0.2 operator CLI")]
pub struct Cli {
    #[arg(long, env = "NIXFLEET_CP_URL", default_value = "https://localhost:8080")]
    pub control_plane_url: String,

    #[arg(long)]
    pub ca_cert: Option<std::path::PathBuf>,

    #[arg(long)]
    pub client_cert: Option<std::path::PathBuf>,

    #[arg(long)]
    pub client_key: Option<std::path::PathBuf>,

    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Show fleet status (hosts + current generation).
    Status,

    /// Walk rollout events for a specific rollout id.
    #[command(name = "rollout")]
    Rollout {
        #[command(subcommand)]
        cmd: RolloutCmd,
    },
}

#[derive(Debug, Subcommand)]
pub enum RolloutCmd {
    Trace { id: String },
}
```

`src/client.rs`:

```rust
use anyhow::Result;
use reqwest::Client;
use std::path::Path;

pub fn build_client(
    ca: Option<&Path>,
    cert: Option<&Path>,
    key: Option<&Path>,
) -> Result<Client> {
    let mut b = Client::builder().use_rustls_tls();
    if let Some(p) = ca {
        b = b.add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read(p)?)?);
    }
    if let (Some(c), Some(k)) = (cert, key) {
        let mut pem = std::fs::read(c)?;
        pem.extend_from_slice(&std::fs::read(k)?);
        b = b.identity(reqwest::Identity::from_pem(&pem)?);
    }
    Ok(b.build()?)
}
```

`src/status.rs`:

```rust
use anyhow::Result;
use comfy_table::Table;
use reqwest::Client;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FleetStatus {
    hosts: Vec<HostStatus>,
    has_verified_artifact: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostStatus {
    hostname: String,
    current_generation_hash: Option<String>,
    last_seen_at: Option<String>,
}

pub async fn run(client: &Client, cp_url: &str) -> Result<()> {
    let s: FleetStatus = client
        .get(format!("{cp_url}/v1/fleet/status"))
        .header("X-Nixfleet-Protocol", "1")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    println!(
        "Control plane has verified artifact: {}",
        if s.has_verified_artifact { "yes" } else { "no" }
    );

    let mut t = Table::new();
    t.set_header(["hostname", "current generation", "last seen"]);
    for h in s.hosts {
        t.add_row([
            h.hostname,
            h.current_generation_hash.unwrap_or_else(|| "-".into()),
            h.last_seen_at.unwrap_or_else(|| "-".into()),
        ]);
    }
    println!("{t}");
    Ok(())
}
```

`src/rollout.rs`:

```rust
use anyhow::Result;
use reqwest::Client;

pub async fn trace(_client: &Client, _cp_url: &str, id: &str) -> Result<()> {
    // Skeleton: CP does not yet expose a rollout trace endpoint. Print
    // a placeholder so the CLI shape is complete. Lands with rollout
    // tables in a later PR.
    println!("rollout trace {id}: no rollout events table in v0.2 skeleton");
    Ok(())
}
```

`src/main.rs`:

```rust
use anyhow::Result;
use clap::Parser;
use nixfleet_cli::cli::{Cli, Cmd, RolloutCmd};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    let client = nixfleet_cli::client::build_client(
        cli.ca_cert.as_deref(),
        cli.client_cert.as_deref(),
        cli.client_key.as_deref(),
    )?;

    match cli.command {
        Cmd::Status => nixfleet_cli::status::run(&client, &cli.control_plane_url).await,
        Cmd::Rollout {
            cmd: RolloutCmd::Trace { id },
        } => nixfleet_cli::rollout::trace(&client, &cli.control_plane_url, &id).await,
    }
}
```

Update `src/lib.rs`:

```rust
pub mod cli;
pub mod client;
pub mod rollout;
pub mod status;
```

- [ ] **Step 4: Write CLI integration tests**

Create `crates/nixfleet-cli/tests/status.rs`:

```rust
use assert_cmd::Command;
use predicates::prelude::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread")]
async fn status_prints_host_table() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/fleet/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "hosts": [{
                "hostname": "h1",
                "currentGenerationHash": "sha256-aa",
                "lastSeenAt": "2026-04-24T12:00:00Z"
            }],
            "hasVerifiedArtifact": true
        })))
        .mount(&server)
        .await;

    let mut cmd = Command::cargo_bin("nixfleet").unwrap();
    cmd.args(["--control-plane-url", &server.uri(), "status"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("h1"))
        .stdout(predicate::str::contains("sha256-aa"));
}

#[test]
fn rollout_trace_prints_placeholder() {
    let mut cmd = Command::cargo_bin("nixfleet").unwrap();
    cmd.args(["--control-plane-url", "http://nowhere", "rollout", "trace", "r-123"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("r-123"));
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p nixfleet-cli 2>&1 | tail -10
cargo test -p nixfleet-control-plane --test routes 2>&1 | tail -5
```

- [ ] **Step 6: Commit**

```bash
git add crates/nixfleet-cli crates/nixfleet-control-plane
git commit -m "feat(cli)(#29): nixfleet status + rollout trace

status fetches GET /v1/fleet/status from the CP over mTLS and renders
a table. rollout trace is a placeholder (full events table lands
with the reconcile-persistence PR). CP gets the /v1/fleet/status
route serving the skeleton fleet overview."
```

### PR 4 acceptance checklist

- [ ] `cargo test -p nixfleet-cli` green.
- [ ] `nixfleet status` prints a table against a wiremock CP.
- [ ] `nixfleet rollout trace <id>` exits 0 with a placeholder message.

---

## Cross-cutting: PR bodies

Every PR body should carry:

- **What's in:** bullet list per crate/file.
- **What's out:** magic rollback (#2, Phase 4), activation (Phase 4), compliance probe execution (separate issue), harness wiring (Phase 2 PR (b)).
- **Contract ties:** CONTRACTS.md §I #1/#2, §II (trust roots), §III (canonicalization), §IV (storage purity), §V (versioning), `docs/trust-root-flow.md §3.2/§3.4/§7`, `rfcs/0003-protocol.md §4/§7`.
- **Tests added:** count + highlights.
- **Reviewer verification block** (build economy — do NOT run these locally):

  ```
  cargo nextest run --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  nix build .#nixfleet-agent .#nixfleet-control-plane .#nixfleet-cli --no-link -L
  nix flake check
  ```

- **Closes:** only the final PR (CLI) should say `Closes #29`. Earlier PRs say `Part of #29`.
- **Language:** English (per `~/.claude-personal/projects/-home-s33d-dev-arcanesys-nixfleet/memory/feedback_pr_language.md`: nixfleet PRs are English).

## Review order

Merge strictly in PR 1 → 2 → 3 → 4 sequence. Each later branch is rebased onto the newly-merged predecessor immediately after merge.

## Rollback / abort

If any PR's review surfaces a fundamental design gap (e.g. the mTLS CN extraction approach doesn't work for `axum-server` without a custom `Acceptor`), pause the stack and post on #29. Do not paper over — the v0.2 skeletons are load-bearing for Phase 2 cross-stream integration.

---

## Self-review notes

- **Spec coverage:** Every kickoff-prompt deliverable (agent poll-only, CP Axum+SQLite+mTLS+4 endpoints, CLI status/trace) maps to tasks. Contract deltas from the coordinator's update (TrustConfig, KeySlot::active_keys signature, verify_artifact reject_before) are in Task 1.5–1.6.
- **Placeholders:** The mTLS peer-cert CN injection referenced "consult git history for the v0.1 auth_cn.rs" rather than inlining the full code — acceptable because the v0.1 code was just deleted and is ~100 LOC of rustls/tokio-rustls glue; re-inlining it here would explode the plan. Executor will reference it via `git show` on the pre-trim commit.
- **Type consistency:** `TrustConfig.ci_release_key: KeySlot` everywhere. `KeySlot::active_keys(&self) -> Vec<TrustedPubkey>` matches Task 1.5 exactly and is the signature `verify_artifact` call sites feed. `verify_artifact(…, reject_before: Option<DateTime<Utc>>)` — consistent across all callers.
- **Spec-gap tasks:** Added `GET /v1/fleet/status` in PR 4 (CLI needs something to hit). Flagged in PR 4 Task 4.1 Step 2 with explicit rationale.
