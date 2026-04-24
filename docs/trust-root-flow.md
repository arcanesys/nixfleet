# Trust-root data flow: `fleet.nix` → CP `verify_artifact`

Describes how Stream B's declarative `nixfleet.trust.*` declarations travel from `fleet.nix` to Stream C's `verify_artifact` call site at the control-plane runtime. Load-bearing Phase 2 design — this wiring is what makes CI-signed `fleet.resolved` actually get verified.

Status: **proposed**, not yet implemented. Lands with the Phase 2 CP integration work.

Cross-references: ARCHITECTURE.md §1.4 (control plane role), CONTRACTS.md §II (trust roots), RFC-0003 §7 (threat model — compromised CP / closure forgery / stale replay).

## 1. Flow at a glance

```
fleet.nix  (Stream B — declarative)
  nixfleet.trust.ciReleaseKey.current = { algorithm = "ecdsa-p256"; public = "<base64>"; };
  nixfleet.trust.ciReleaseKey.previous = null;        # 30-day rotation grace
  nixfleet.trust.ciReleaseKey.rejectBefore = null;    # compromise switch
  nixfleet.trust.atticCacheKey.current = "attic:cache.lab.internal:<base64>";
  nixfleet.trust.orgRootKey.current = null;
       │
       │  NixOS module system: modules/_trust.nix typechecks + asserts
       ▼
CP-host NixOS config  (applied via mkHost on the CP host)
  environment.etc."nixfleet/cp/trust.json".source = writers.writeJSON "trust.json" {
    ciReleaseKey = { current = { algorithm; public; }; previous = null; rejectBefore = null; };
    atticCacheKey = { current = "..."; previous = null; rejectBefore = null; };
    orgRootKey = { current = null; previous = null; rejectBefore = null; };
  };
  systemd.services.nixfleet-control-plane.serviceConfig.ExecStart
    += [ "--trust-file" "/etc/nixfleet/cp/trust.json" ];
       │
       │  systemd launches the binary with the flag
       ▼
CP binary  (Stream C — crates/control-plane)
  Cli::parse() → args.trust_file: PathBuf
  main() reads /etc/nixfleet/cp/trust.json, deserializes (serde_json) into
    proto::TrustConfig { ci_release_key: KeySlot, attic_cache_key: KeySlot, org_root_key: KeySlot }
  On every fleet.resolved load (tick or webhook):
    let trust_roots: Vec<TrustedPubkey> = trust_config.ci_release_key.active_keys(now);
    reconciler::verify_artifact(bytes, signature, &trust_roots, now, freshness_window)
```

## 2. Why this shape

Three invariants drive the design.

**(a) No Nix-to-runtime bridge except JSON-on-disk.** The CP binary is a separate process from the NixOS module system. It cannot ask the config tree directly at runtime. The only stable carrier between Nix and a running service is a file on disk, written by the module, consumed by the binary. This matches the existing pattern for `--db-path`, `--tls-cert`, etc.

**(b) Zero-knowledge CP (CONTRACTS.md §IV, RFC-0003 §7).** The CP MUST be reconstructible from git + agent check-ins. The trust config is derivable from `fleet.nix` — rebuilding the CP host from an empty state regenerates `/etc/nixfleet/cp/trust.json` on activation. No state is lost, no state needs to persist across teardowns.

**(c) Rotation without redeploy.** `KeySlot.current` + `KeySlot.previous` both present means both keys are active. The CP's `active_keys(now)` returns both until `rejectBefore` is exceeded or `previous` is cleared. This lets Stream A rotate the CI release key by:
1. Generating a new key (may be a different algorithm).
2. Setting `current = <new>`, `previous = <old>` in `fleet.nix`.
3. CI starts signing with the new key.
4. After 30 days (or when `fleet.nix` clears `previous`), the CP stops accepting old-key-signed artifacts.

No CP restart needed — the trust file regenerates on next activation; the CP rereads on tick. (Implementation choice: re-read every N ticks, or use inotify, or restart on config change. Pick in the implementation PR.)

## 3. Per-hop specification

### 3.1 Declaration surface — `nixfleet.trust.*`

Already landed (PR #17, reinforced by PR #18 contract amendment). See `modules/_trust.nix`:

```nix
nixfleet.trust.ciReleaseKey.current = {
  algorithm = "ecdsa-p256";  # or "ed25519"
  public = "<base64 raw pubkey bytes>";
};
```

Submodule shape per CONTRACTS.md §II #1. Enum validates at eval time. Assertions enforce `.previous` only when `.current` is set.

### 3.2 NixOS module → `/etc/nixfleet/cp/trust.json`

New addition to `modules/scopes/nixfleet/_control-plane.nix`. Read-through of `config.nixfleet.trust`:

```nix
let
  trustJson = pkgs.writers.writeJSON "trust.json" {
    ciReleaseKey = config.nixfleet.trust.ciReleaseKey;
    atticCacheKey = config.nixfleet.trust.atticCacheKey;
    orgRootKey = config.nixfleet.trust.orgRootKey;
  };
in {
  environment.etc."nixfleet/cp/trust.json".source = trustJson;

  systemd.services.nixfleet-control-plane.serviceConfig.ExecStart = lib.mkForce (
    lib.concatStringsSep " " (existingArgs ++ [
      "--trust-file" "/etc/nixfleet/cp/trust.json"
    ])
  );
}
```

The file is world-readable (contains only public keys — by definition not secret). `atticCacheKey` and `orgRootKey` both emit as `{current, previous, rejectBefore}` slot objects: key material stays in its algorithm's native format (attic-native `"attic:<host>:<base64>"` strings under `atticCacheKey`; typed `{algorithm, public}` submodules under `orgRootKey`), but both slots expose the same rotation-grace + compromise-switch surface that `ciReleaseKey` uses.

### 3.3 CP binary CLI surface

Add to `crates/control-plane/src/cli.rs`:

```rust
#[derive(Parser)]
struct Cli {
    // ... existing flags ...

    /// Path to the trust-root JSON file (see docs/trust-root-flow.md §3.2).
    /// Required. Contains the declared CI release key + attic cache key + org root key.
    #[arg(long, value_name = "PATH")]
    trust_file: PathBuf,
}
```

On startup, `main()`:

```rust
let trust_config: proto::TrustConfig = serde_json::from_str(
    &std::fs::read_to_string(&args.trust_file)?,
)?;
```

### 3.4 `proto::TrustConfig` shape

Add to `crates/nixfleet-proto`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustConfig {
    /// Contract version of this file. Bumped only on breaking schema
    /// changes; binaries refuse to start on unknown versions (see §7.5).
    pub schema_version: u32,

    pub ci_release_key: KeySlot,
    #[serde(default)]
    pub attic_cache_key: Option<AtticKeySlot>,
    #[serde(default)]
    pub org_root_key: Option<KeySlot>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeySlot {
    #[serde(default)]
    pub current: Option<TrustedPubkey>,      // TrustedPubkey already exists (PR #20)
    #[serde(default)]
    pub previous: Option<TrustedPubkey>,
    /// Compromise switch (§7.2): artifacts with `signedAt < rejectBefore`
    /// are refused regardless of which key signed them — applies to both
    /// `current` and `previous` slots.
    #[serde(default)]
    pub reject_before: Option<DateTime<Utc>>,
}

impl KeySlot {
    /// Returns the active key list for `now` — used as the `&[TrustedPubkey]`
    /// slice passed to `verify_artifact`. `rejectBefore` is not enforced
    /// here; that check happens inside `verify_artifact` against the
    /// artifact's `signedAt` (see §3.5 and §7.2).
    pub fn active_keys(&self) -> Vec<TrustedPubkey> {
        let mut keys = Vec::new();
        if let Some(k) = &self.current { keys.push(k.clone()); }
        if let Some(k) = &self.previous { keys.push(k.clone()); }
        keys
    }
}
```

`AtticKeySlot` mirrors `KeySlot`'s shape (`current` / `previous` / `rejectBefore`) with an `AtticPubkey(String)` newtype holding attic-native `"attic:<host>:<base64>"` material instead of a `{algorithm, public}` pair. Agents forward the raw string to attic tooling at closure-verify time.

### 3.5 Verify call site

In the CP's reconciler tick handler (new Phase 2 code):

```rust
let ci_slot = &trust_config.ci_release_key;
let ci_keys = ci_slot.active_keys();
let verified = reconciler::verify_artifact(
    fleet_resolved_bytes,
    signature_bytes,
    &ci_keys,
    now,
    freshness_window,           // from fleet.resolved, per RFC-0002 §4 step 0
    ci_slot.reject_before,      // compromise switch, §7.2
)?;
```

Fail closed: any `VerifyError` variant aborts the reconcile tick. CP logs the failure; subsequent ticks retry (artifact may change, rotation may land, freshness may reset). `verify_artifact` gains a new `RejectedBeforeTimestamp { signed_at, reject_before }` variant, distinct from `Stale` — semantic difference is operator-declared incident response vs routine expiry.

## 4. `fleet.resolved.json` distribution

Parallel question: how does the artifact itself reach the CP?

Per ARCHITECTURE.md §1.4: CP polls the git forge for channel-ref updates. Two concrete implementations possible:

**(a) CP pulls from Forgejo raw-file HTTP.** Requires the CP to have Forgejo HTTP access (lab.internal). Requires a new CLI flag `--release-url-template https://git.lab.internal/<owner>/fleet/raw/main/releases/fleet.resolved.json` or similar.

**(b) CP reads from a local git checkout.** The CP host runs a systemd timer that `git pull`s a shared fleet clone into `/var/lib/nixfleet-cp/fleet.git/`; CP reads `/var/lib/nixfleet-cp/fleet.git/releases/fleet.resolved.json`. No new network surface on the CP binary.

**(c) Agents fetch directly from attic + CP serves the artifact as a pass-through.** Agents resolve closure hashes out-of-band. CP only serves the artifact as "here is what I was told to serve". Matches the "caching router" framing in ARCHITECTURE.md.

Recommendation: start with (b) — simplest, no new network surface on CP, matches the "git is the trust root" invariant. Revisit in Phase 3 if operations need push-based updates.

This is a separate design question from the trust-root flow but needs resolution before Phase 2 can end-to-end test. Open as a follow-up issue on nixfleet once the trust-flow implementation PR is posted.

## 5. Agent-side parity

Agents also verify closures (against `atticCacheKey`). The same pattern applies:

- Agent NixOS module writes `/etc/nixfleet/agent/trust.json` from `config.nixfleet.trust`.
- Agent binary gets `--trust-file` flag.
- Agent calls into `verify_artifact` (or an attic-specific variant) before activation.

Differences from CP:
- Agents don't need the full `ciReleaseKey` — only `atticCacheKey` (for closure signatures). The shared trust file carries both; the agent ignores the ci key unless it ever direct-fetches `fleet.resolved` (fallback path — ARCHITECTURE.md §1.4 "agents (fallback direct fetch)").
- Per-host `trust.json` is identical across the fleet (same keys everywhere). Could be factored into a single `trust.json.d/` if symlinks get annoying.

## 6. Rotation walkthrough (worked example)

Starting state: `ciReleaseKey.current = { algorithm = "ecdsa-p256"; public = "K1"; }`, no previous.

Operator rotates to a new ed25519 key:

1. Stream A generates the new keypair (e.g. new YubiKey, or software-held ed25519 since M70q's TPM constraint only affects HSM-backed signing).
2. Operator edits `fleet.nix`:
   ```nix
   nixfleet.trust.ciReleaseKey = {
     current = { algorithm = "ed25519"; public = "K2"; };
     previous = { algorithm = "ecdsa-p256"; public = "K1"; };
   };
   ```
3. Commit + CI build. CI starts signing with K2 on the next pipeline.
4. Deploy the CP host. New `/etc/nixfleet/cp/trust.json` contains both K1 and K2.
5. On next tick, `active_keys(now)` returns `[K2_ed25519, K1_ecdsa-p256]`. CP verifies both old-key-signed and new-key-signed artifacts.
6. After 30 days, operator edits `fleet.nix` again:
   ```nix
   nixfleet.trust.ciReleaseKey = {
     current = { algorithm = "ed25519"; public = "K2"; };
     # previous cleared
   };
   ```
7. Deploy. K1-signed artifacts are now rejected as unknown-key.

Cross-algorithm rotation is supported end-to-end — Stream C's `verify_artifact` already iterates the slice and matches on each entry's `algorithm` tag (PR #20).

## 7. Decisions (locked for the implementation PR)

### 7.1 Reload model

**Decision: restart-only.** CP has no SIGHUP handler and no file-watcher. Trust rotation requires a service restart, which `nixos-rebuild switch` triggers for free when `/etc/nixfleet/cp/trust.json` content changes.

Rationale. Rotation is rare (30-day grace per CONTRACTS §II #1) and a 5–10s CP bounce under a 24h freshness window is irrelevant. File-watching code is a real failure surface we do not need to build speculatively. Agents' check-ins during the restart retry per RFC-0003 §8 offline grace — zero behavioral impact.

### 7.2 `rejectBefore` semantics

**Decision: applies to both `current` and `previous`.** Any artifact whose `meta.signedAt < rejectBefore` is refused regardless of which key signed it.

Rationale. CONTRACTS §II #1 "Compromise response" reads "all artifacts signed before that are refused regardless of key." Making it current-only would re-purpose it as rotation-window control, which `.previous` grace already covers. `rejectBefore` is the compromise-incident switch; it belongs at slot level, not per-key.

Implementation lives in `verify_artifact` (not in `KeySlot::active_keys`) so the error path can distinguish `RejectedBeforeTimestamp { signed_at, reject_before }` from routine `Stale`.

### 7.3 File atomicity

**Decision: NixOS atomic swap is sufficient for v0.2.** `environment.etc` routes through `nix-store`-linked files; the swap at activation time is atomic at the VFS layer.

Non-NixOS deployments would need a `rename(2)` dance in the write path. That is deferred — nixfleet does not target non-NixOS operator hosts in v0.2.

### 7.4 Trust config `schemaVersion`

**Decision: required `schemaVersion: 1` at the top level.** CP and agent binaries validate on startup; unknown version → refuse to start with an actionable error.

Rationale. Matches CONTRACTS §V's per-contract versioning pattern. Fail-fast beats silent misinterpretation. The cost is one `u32`.

Evolution rule:
- Adding optional fields stays at `schemaVersion: 1` (serde-default handles absence).
- Removing fields, changing meaning of existing fields, or changing the required/optional posture of a field → bump to `schemaVersion: 2` with a dual-read migration window (binary accepts both during the window).
- `rejectBefore` is operator-managed data, not a schema concern — changing its value does not require a version bump.

## 8. Summary

- `fleet.nix` declares pubkeys via the typed `nixfleet.trust.*` option tree (PR #17).
- CP-host NixOS module materializes the declaration as `/etc/nixfleet/cp/trust.json` at activation.
- CP binary reads the file via `--trust-file` flag, deserializes into `proto::TrustConfig`, calls `slot.active_keys(now)` to get the `&[TrustedPubkey]` slice that `verify_artifact` wants.
- Rotation works declaratively — both `current` and `previous` active until `rejectBefore` clears the overlap.
- Agents get the same pattern via `/etc/nixfleet/agent/trust.json`.
- `fleet.resolved.json` distribution to the CP is a separate open question — recommendation is local-git-checkout pattern (b).

Implementation PR on nixfleet comes after the #5 harness scaffold's TODO(5) slot-in lands. Suggested scope: `modules/scopes/nixfleet/_control-plane.nix` trust-file wiring + `crates/nixfleet-proto::TrustConfig` + `crates/control-plane` CLI flag + a harness scenario that asserts the CP refuses artifacts signed by a non-declared key.
